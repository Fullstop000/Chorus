use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use super::ChatBridge;

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
    async fn get_or_create_service(
        &self,
        agent_key: &str,
    ) -> StreamableHttpService<ChatBridge, LocalSessionManager> {
        // Fast path — read lock only.
        {
            let guard = self.services.read().await;
            if let Some(svc) = guard.get(agent_key) {
                return svc.clone();
            }
        }

        // Slow path — upgrade to write lock and insert.
        let mut guard = self.services.write().await;
        // Double-check after acquiring the write lock.
        if let Some(svc) = guard.get(agent_key) {
            return svc.clone();
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
        svc
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
    let service = server.get_or_create_service(&agent_key).await;
    let response = service.handle(request).await;
    response.into_response()
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the shared bridge HTTP server.
///
/// Agents connect via `http://<listen_addr>/<agent_key>/mcp`. Each agent_key
/// gets its own `StreamableHttpService` (and thus its own MCP session pool).
pub async fn run_bridge_server(listen_addr: &str, server_url: &str) -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let server = Arc::new(BridgeServer::new(server_url.to_string(), ct.clone()));

    let app = Router::new()
        .route("/{agent_key}/mcp", axum::routing::any(handle_mcp))
        .route(
            "/health",
            axum::routing::get(|| async { "ok" }),
        )
        .with_state(server);

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let port = listener.local_addr()?.port();

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

    Ok(())
}
