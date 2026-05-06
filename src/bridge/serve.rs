use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio_util::sync::CancellationToken;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::any, Router};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use super::agent_key_is_safe;
use super::ChatBridge;

// ---------------------------------------------------------------------------
// BridgeServer — single shared service for all agents
// ---------------------------------------------------------------------------

struct BridgeServer {
    service: StreamableHttpService<ChatBridge, LocalSessionManager>,
}

impl BridgeServer {
    fn new(server_url: String, cancellation_token: CancellationToken) -> Self {
        let url = server_url.clone();
        let config = StreamableHttpServerConfig {
            cancellation_token: cancellation_token.child_token(),
            ..Default::default()
        };

        let session_manager = LocalSessionManager {
            sessions: Default::default(),
            session_config: rmcp::transport::streamable_http_server::session::local::SessionConfig {
                channel_capacity: rmcp::transport::streamable_http_server::session::local::SessionConfig::DEFAULT_CHANNEL_CAPACITY,
                keep_alive: Some(Duration::from_secs(300)),
            },
        };

        let service = StreamableHttpService::new(
            move || Ok(ChatBridge::new(url.clone())),
            Arc::new(session_manager),
            config,
        );

        Self { service }
    }
}

// ---------------------------------------------------------------------------
// Axum handler — validates X-Agent-Id header and delegates to shared service
// ---------------------------------------------------------------------------

/// Extract agent key from `X-Agent-Id` header or `Authorization: Bearer <key>`.
fn extract_agent_key_from_request(request: &Request<axum::body::Body>) -> Option<&str> {
    // Prefer X-Agent-Id
    if let Some(key) = request
        .headers()
        .get("X-Agent-Id")
        .and_then(|v| v.to_str().ok())
    {
        return Some(key);
    }
    // Fallback: Authorization: Bearer <key>
    if let Some(auth) = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return Some(token);
        }
    }
    None
}

async fn handle_mcp(
    State(server): State<Arc<BridgeServer>>,
    mut request: Request<axum::body::Body>,
) -> Response {
    // Validate agent identity header is present and safe.
    let agent_key = match extract_agent_key_from_request(&request) {
        Some(key) if agent_key_is_safe(key) => key.to_string(),
        _ => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("Content-Type", "text/plain")
                .body(axum::body::Body::from(
                    "Invalid or missing X-Agent-Id header",
                ))
                .expect("valid response");
        }
    };

    // Ensure X-Agent-Id is set on the request so ChatBridge can read it
    // from Extension<Parts> regardless of whether the client used the
    // native header or the bearer-token fallback.
    request
        .headers_mut()
        .insert("X-Agent-Id", agent_key.parse().unwrap());

    let response = server.service.handle(request).await;
    response.into_response()
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Build the bridge Axum router without binding or writing discovery info.
///
/// Useful for tests that want to plug the router into their own listener.
pub fn build_bridge_router(server_url: &str) -> (Router, CancellationToken) {
    let ct = CancellationToken::new();
    let server = Arc::new(BridgeServer::new(server_url.to_string(), ct.clone()));

    let app = Router::new()
        .route("/mcp", any(handle_mcp))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(server);

    (app, ct)
}

/// Start the shared bridge HTTP server.
///
/// Agents connect via `http://<listen_addr>/mcp` with the `X-Agent-Id` header.
pub async fn run_bridge_server(listen_addr: &str, server_url: &str) -> anyhow::Result<()> {
    let (app, ct) = build_bridge_router(server_url);

    // Resolve listen_addr before binding so we can reject non-loopback addresses
    // without ever opening a listening socket on them.
    let resolved: Vec<std::net::SocketAddr> = listen_addr
        .to_socket_addrs()
        .with_context(|| format!("invalid listen address: {}", listen_addr))?
        .collect();
    if resolved.is_empty() {
        anyhow::bail!("listen address {} resolved to no sockets", listen_addr);
    }
    // The MCP bridge has no authentication beyond the loopback bind — refuse to
    // expose it beyond localhost. Cross-machine MCP must go through `chorus
    // bridge`, which proxies tool-calls over the authenticated WS upgrade.
    for addr in &resolved {
        if !addr.ip().is_loopback() {
            anyhow::bail!(
                "Bridge refuses to bind to non-loopback address {}. The MCP bridge has no authentication; \
                 only localhost binds are supported. For cross-machine, use `chorus bridge` instead.",
                addr
            );
        }
    }

    let listener = tokio::net::TcpListener::bind(&resolved[..]).await?;
    let port = listener.local_addr()?.port();

    // Write discovery info so drivers can find this bridge.
    crate::bridge::discovery::write_bridge_info(&crate::bridge::discovery::BridgeInfo {
        port,
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    })?;
    // Guard ensures the discovery file is removed on every exit path —
    // normal shutdown, early error return, or panic.
    let _discovery_guard = crate::bridge::discovery::DiscoveryGuard;

    tracing::info!(port, "bridge server listening");

    // Graceful shutdown on ctrl-c.
    let ct_shutdown = ct.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        ct_shutdown.cancel();
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { ct.cancelled().await })
        .await?;

    Ok(())
}
