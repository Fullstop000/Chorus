use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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
///
/// When the cap is reached, the least-recently-used entry is evicted to make
/// room for new agents. In-flight MCP sessions on the evicted entry are
/// dropped — acceptable because eviction only happens when 128+ distinct
/// agent_keys are active, which is unusual for a single host.
const MAX_SERVICES: usize = 128;

// ---------------------------------------------------------------------------
// BridgeServer — shared state for all per-agent services
// ---------------------------------------------------------------------------

/// One entry in the services map. Tracks last-access time for LRU eviction
/// when the map reaches `MAX_SERVICES`.
struct ServiceEntry {
    service: StreamableHttpService<ChatBridge, LocalSessionManager>,
    last_accessed: Instant,
}

/// Shared state for the bridge HTTP server.
///
/// Lazily creates one `StreamableHttpService<ChatBridge>` per agent_key. Each
/// service manages its own MCP sessions via `LocalSessionManager`.
struct BridgeServer {
    server_url: String,
    cancellation_token: CancellationToken,
    services: RwLock<HashMap<String, ServiceEntry>>,
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
    /// Uses LRU eviction when the services map is at `MAX_SERVICES` capacity:
    /// the entry with the oldest `last_accessed` is dropped to make room.
    /// Every access (hit or miss) refreshes `last_accessed`.
    async fn get_or_create_service(
        &self,
        agent_key: &str,
    ) -> StreamableHttpService<ChatBridge, LocalSessionManager> {
        // Fast path — try a read lock first, but we still need to bump
        // `last_accessed`, which requires a write lock. A hit on the read
        // lock tells us we don't need to create, but we fall through to
        // update the timestamp under write lock. In practice the extra
        // contention is negligible; if it ever matters, switch to atomic
        // timestamps on the entry.
        let mut guard = self.services.write().await;

        if let Some(entry) = guard.get_mut(agent_key) {
            entry.last_accessed = Instant::now();
            return entry.service.clone();
        }

        // Cache miss. Evict LRU entry if at capacity.
        if guard.len() >= MAX_SERVICES {
            if let Some(evicted_key) = guard
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(k, _)| k.clone())
            {
                guard.remove(&evicted_key);
                tracing::info!(
                    evicted_key,
                    new_key = agent_key,
                    "evicted LRU MCP service to make room"
                );
            }
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

        guard.insert(
            agent_key.to_owned(),
            ServiceEntry {
                service: svc.clone(),
                last_accessed: Instant::now(),
            },
        );
        tracing::info!(agent_key, "created MCP service for agent");
        svc
    }
}

// ---------------------------------------------------------------------------
// Axum handler — forwards requests to the per-agent StreamableHttpService
// ---------------------------------------------------------------------------

/// Reject agent_keys that could pivot to other endpoints or traverse the
/// filesystem, but accept the full set of names Chorus allows elsewhere.
///
/// Chorus only enforces `!name.is_empty()` at create time (see
/// `handle_create_agent` in `src/server/handlers/agents.rs`), so existing
/// agents may have spaces, dots, or Unicode in their names. Axum URL-decodes
/// the path segment before we see it, so drivers that URL-encode names get
/// the original back here.
fn agent_key_is_safe(key: &str) -> bool {
    !(key.is_empty()
        || key.len() > 256
        || key.contains('/')
        || key.contains('\\')
        || key.contains("..")
        || key.chars().any(|c| c.is_control()))
}

/// Axum handler: extract agent_key from the path, look up (or create) the
/// corresponding `StreamableHttpService`, and delegate the HTTP request to it.
async fn handle_mcp(
    Path(agent_key): Path<String>,
    State(server): State<Arc<BridgeServer>>,
    request: Request<axum::body::Body>,
) -> Response {
    if !agent_key_is_safe(&agent_key) {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(axum::body::Body::from(
                "Invalid agent_key: must be non-empty, <=256 chars, no path separators or control characters",
            ))
            .expect("valid response");
    }

    let service = server.get_or_create_service(&agent_key).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_key_accepts_chorus_names() {
        // Names Chorus currently allows through handle_create_agent.
        assert!(agent_key_is_safe("bot1"));
        assert!(agent_key_is_safe("Agent Smith"));
        assert!(agent_key_is_safe("bot.with.dots"));
        assert!(agent_key_is_safe("unicode-名字"));
        assert!(agent_key_is_safe("a"));
        assert!(agent_key_is_safe(&"x".repeat(256)));
    }

    #[test]
    fn agent_key_rejects_dangerous_input() {
        assert!(!agent_key_is_safe(""));
        assert!(!agent_key_is_safe(&"x".repeat(257)));
        assert!(!agent_key_is_safe("../etc/passwd"));
        assert!(!agent_key_is_safe("a/b"));
        assert!(!agent_key_is_safe("a\\b"));
        assert!(!agent_key_is_safe("with\0null"));
        assert!(!agent_key_is_safe("with\nnewline"));
        assert!(!agent_key_is_safe(".."));
    }
}
