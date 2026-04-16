use anyhow::Context;
use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde::Deserialize;
use serde_json::json;

use super::pairing::PairingTokenStore;
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
    /// One-time pairing tokens minted via `/admin/pair`.
    pairing_tokens: PairingTokenStore,
    /// Cached token -> agent_key mapping kept alive for the session lifetime.
    ///
    /// Once a token is consumed on the first MCP request, we memoize it here
    /// because rmcp's `StreamableHttpService` keeps routing subsequent
    /// requests for that session to the same URL path — if we dropped the
    /// mapping after consume, the second request would 401. Memory grows
    /// without bound until bridge restart, which is acceptable for Phase 2;
    /// Phase 3 plans a proper session-scoped store.
    ///
    // TODO(Phase 3): token_to_agent grows without bound — every successful
    // pairing adds an entry that is never removed. Acceptable for a local
    // daemon that restarts periodically (restart drops the map), but needs
    // real session-lifecycle eviction before multi-machine / long-running
    // deployments. Likely fix: tie entries to rmcp session lifetime and
    // drop them when the session closes.
    token_to_agent: RwLock<HashMap<String, String>>,
}

impl BridgeServer {
    fn with_token_ttl(
        server_url: String,
        cancellation_token: CancellationToken,
        token_ttl: Option<Duration>,
    ) -> Self {
        let pairing_tokens = match token_ttl {
            Some(ttl) => PairingTokenStore::with_ttl(ttl),
            None => PairingTokenStore::new(),
        };
        Self {
            server_url,
            cancellation_token,
            services: RwLock::default(),
            pairing_tokens,
            token_to_agent: RwLock::default(),
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
        // At ≤128 agents on localhost, write-lock contention is negligible, so
        // we take the write lock unconditionally. If per-request access becomes
        // a contention hotspot later, switch last_accessed to an AtomicU64 so
        // cache hits can stay on a read lock.
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

/// Axum handler: resolve a pairing token to its bound agent_key, then delegate
/// to that agent's `StreamableHttpService`.
///
/// The first request on a token *consumes* it and caches the mapping in
/// `token_to_agent`. Subsequent requests on the same URL reuse the cached
/// mapping — necessary because MCP clients keep POSTing to the init URL for
/// the lifetime of the session.
async fn handle_mcp_token(
    Path(token): Path<String>,
    State(server): State<Arc<BridgeServer>>,
    request: Request<axum::body::Body>,
) -> Response {
    // Check the session-lifetime cache first — avoids the consume roundtrip
    // on every subsequent request for the same session.
    let cached = server.token_to_agent.read().await.get(&token).cloned();

    let agent_key = if let Some(key) = cached {
        key
    } else {
        // First request on this token — try to consume it.
        match server.pairing_tokens.consume(&token).await {
            Some(key) => {
                server
                    .token_to_agent
                    .write()
                    .await
                    .insert(token.clone(), key.clone());
                key
            }
            None => return unauthorized_response(),
        }
    };

    let service = server.get_or_create_service(&agent_key).await;
    let response = service.handle(request).await;
    response.into_response()
}

fn unauthorized_response() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(axum::body::Body::from("Invalid or expired pairing token"))
        .expect("valid response")
}

#[derive(Deserialize)]
struct PairRequest {
    agent_key: String,
}

/// Admin endpoint: mint a one-time pairing token bound to `agent_key`.
///
/// Protected by the loopback bind check in `run_bridge_server` — Phase 2
/// does not support remote callers, so there is no auth beyond that.
async fn handle_pair(
    State(server): State<Arc<BridgeServer>>,
    Json(req): Json<PairRequest>,
) -> Response {
    if !agent_key_is_safe(&req.agent_key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid agent_key"})),
        )
            .into_response();
    }
    let token = server.pairing_tokens.issue(req.agent_key).await;
    Json(json!({"token": token})).into_response()
}

// ---------------------------------------------------------------------------
// Discovery cleanup guard
// ---------------------------------------------------------------------------

/// Removes the bridge discovery file when dropped.
///
/// Placed in `run_bridge_server` after `write_bridge_info` so that the file is
/// cleaned up on every exit path — normal shutdown, error return, or panic.
struct DiscoveryGuard;

impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        crate::bridge::discovery::remove_bridge_info();
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Build the bridge Axum router without binding or writing discovery info.
///
/// Useful for tests that want to plug the router into their own listener.
pub fn build_bridge_router(server_url: &str) -> (Router, CancellationToken) {
    build_bridge_router_with_token_ttl(server_url, None)
}

/// Same as [`build_bridge_router`] but with an override for the pairing-token
/// TTL. Tests use this to exercise the expired-token path without sleeping
/// for the 5-minute default.
pub fn build_bridge_router_with_token_ttl(
    server_url: &str,
    token_ttl: Option<Duration>,
) -> (Router, CancellationToken) {
    let ct = CancellationToken::new();
    let server = Arc::new(BridgeServer::with_token_ttl(
        server_url.to_string(),
        ct.clone(),
        token_ttl,
    ));

    let app = Router::new()
        .route("/{agent_key}/mcp", axum::routing::any(handle_mcp))
        .route("/token/{token}/mcp", axum::routing::any(handle_mcp_token))
        .route("/admin/pair", axum::routing::post(handle_pair))
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

    // Resolve listen_addr before binding so we can reject non-loopback addresses
    // without ever opening a listening socket on them.
    let resolved: Vec<std::net::SocketAddr> = listen_addr
        .to_socket_addrs()
        .with_context(|| format!("invalid listen address: {}", listen_addr))?
        .collect();
    if resolved.is_empty() {
        anyhow::bail!("listen address {} resolved to no sockets", listen_addr);
    }
    // Phase 1 has no authentication — refuse to expose the bridge beyond
    // loopback. A user passing `--listen 0.0.0.0:4321` would otherwise silently
    // accept unauthenticated network traffic.
    for addr in &resolved {
        if !addr.ip().is_loopback() {
            anyhow::bail!(
                "Bridge refuses to bind to non-loopback address {}. Phase 1 bridge has no authentication; \
                 only localhost binds are supported. If you need non-loopback binding, wait for Phase 2 pairing tokens.",
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
    let _discovery_guard = DiscoveryGuard;

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
