use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use super::ChatBridge;

/// Max number of per-agent services the bridge will cache simultaneously.
///
/// Each entry corresponds to a `StreamableHttpService<ChatBridge>` plus its
/// MCP session pool. Capping protects against unbounded memory growth when a
/// misbehaving client cycles through fresh agent_keys.
const MAX_SERVICES: usize = 128;

/// Allowed shape for `agent_key` path segments.
///
/// The key is interpolated into internal URLs (`/internal/agent/{key}`) so it
/// must not carry path separators, query fragments, dot-segments, or any
/// characters that would let an attacker pivot to a different endpoint.
static AGENT_KEY_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9_-]{1,64}$").expect("valid regex"));

// ---------------------------------------------------------------------------
// BridgeServer — shared state for all per-agent services
// ---------------------------------------------------------------------------

/// Shared state for the bridge HTTP server.
///
/// Lazily creates one `StreamableHttpService<ChatBridge>` per agent_key. Each
/// service manages its own MCP sessions via `LocalSessionManager`.
struct BridgeServer {
    server_url: String,
    cancellation_token: CancellationToken,
    services: RwLock<HashMap<String, StreamableHttpService<ChatBridge, LocalSessionManager>>>,
}

impl BridgeServer {
    fn new(server_url: String, cancellation_token: CancellationToken) -> Self {
        Self {
            server_url,
            cancellation_token,
            services: RwLock::default(),
        }
    }

    /// Get an existing service for `agent_key`, or create one on the fly.
    ///
    /// Returns `None` when the services map is at `MAX_SERVICES` capacity and
    /// the requested key isn't already cached. Callers should surface this as
    /// a 503 so the client can retry later.
    async fn get_or_create_service(
        &self,
        agent_key: &str,
    ) -> Option<StreamableHttpService<ChatBridge, LocalSessionManager>> {
        // Fast path — read lock only.
        {
            let guard = self.services.read().await;
            if let Some(svc) = guard.get(agent_key) {
                return Some(svc.clone());
            }
        }

        // Slow path — upgrade to write lock and insert.
        let mut guard = self.services.write().await;
        // Double-check after acquiring the write lock.
        if let Some(svc) = guard.get(agent_key) {
            return Some(svc.clone());
        }

        // Enforce the cap on new entries only; existing entries above the cap
        // (if the limit were ever lowered) continue to serve.
        if guard.len() >= MAX_SERVICES {
            tracing::warn!(
                agent_key,
                cached = guard.len(),
                max = MAX_SERVICES,
                "refusing to create MCP service: services map at capacity"
            );
            return None;
        }

        let key = agent_key.to_owned();
        let url = self.server_url.clone();
        let config = StreamableHttpServerConfig {
            cancellation_token: self.cancellation_token.child_token(),
            ..Default::default()
        };

        let svc = StreamableHttpService::new(
            move || Ok(ChatBridge::new(key.clone(), url.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );

        guard.insert(agent_key.to_owned(), svc.clone());
        tracing::info!(agent_key, "created MCP service for agent");
        Some(svc)
    }
}

// ---------------------------------------------------------------------------
// Axum handler — forwards requests to the per-agent StreamableHttpService
// ---------------------------------------------------------------------------

/// Axum handler: extract agent_key from the path, look up (or create) the
/// corresponding `StreamableHttpService`, and delegate the HTTP request to it.
async fn handle_mcp(
    Path(agent_key): Path<String>,
    State(server): State<Arc<BridgeServer>>,
    request: Request<axum::body::Body>,
) -> Response {
    // Reject keys that could be used to pivot to other Chorus endpoints.
    // The key is interpolated into `/internal/agent/{key}` downstream.
    if !AGENT_KEY_REGEX.is_match(&agent_key) {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(axum::body::Body::from(
                "Invalid agent_key: must match [A-Za-z0-9_-]{1,64}",
            ))
            .expect("valid response");
    }

    let Some(service) = server.get_or_create_service(&agent_key).await else {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(axum::body::Body::from(
                "Bridge service capacity exhausted; try again later",
            ))
            .expect("valid response");
    };
    let response = service.handle(request).await;
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
        .route("/{agent_key}/mcp", axum::routing::any(handle_mcp))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(server);

    (app, ct)
}

/// Start the shared bridge HTTP server.
///
/// Agents connect via `http://<listen_addr>/<agent_key>/mcp`. Each agent_key
/// gets its own `StreamableHttpService` (and thus its own MCP session pool).
pub async fn run_bridge_server(listen_addr: &str, server_url: &str) -> anyhow::Result<()> {
    let (app, ct) = build_bridge_router(server_url);

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;

    // Phase 1 has no authentication — refuse to expose the bridge beyond
    // loopback. A user passing `--listen 0.0.0.0:4321` would otherwise silently
    // accept unauthenticated network traffic.
    if !local_addr.ip().is_loopback() {
        anyhow::bail!(
            "Bridge refuses to bind to non-loopback address {}. Phase 1 bridge has no authentication; \
             only localhost binds are supported. If you need non-loopback binding, wait for Phase 2 pairing tokens.",
            local_addr
        );
    }

    let port = local_addr.port();

    // Write discovery info so drivers can find this bridge.
    crate::bridge::discovery::write_bridge_info(&crate::bridge::discovery::BridgeInfo {
        port,
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    })?;

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

    // Clean up discovery file on shutdown
    crate::bridge::discovery::remove_bridge_info();

    Ok(())
}
