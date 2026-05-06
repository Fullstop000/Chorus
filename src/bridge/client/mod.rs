//! Bridge client: the runtime side of the bridge↔platform split. The
//! platform exposes `GET /api/bridge/ws` and the chat HTTP API at
//! `platform_http`; this module dials both, runs an [`AgentManager`]
//! locally, and reconciles desired state from `bridge.target` frames.

mod local_store;
mod reconcile;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use crate::store::Store;

#[derive(Clone)]
pub struct BridgeClientConfig {
    pub platform_ws: String,
    pub platform_http: String,
    pub token: Option<String>,
    pub machine_id: String,
    pub bridge_listen: String,
    pub agents_dir: PathBuf,
    pub store: Arc<Store>,
}

pub async fn run_bridge_client(cfg: BridgeClientConfig) -> anyhow::Result<()> {
    use crate::agent::manager::AgentManager;
    use crate::server::event_bus::EventBus;
    use tokio_util::sync::CancellationToken;

    let event_bus = Arc::new(EventBus::new());

    // 1. Bind embedded MCP bridge for local agents to reach the platform's HTTP API.
    let (bridge_app, bridge_ct) = crate::bridge::serve::build_bridge_router(&cfg.platform_http);
    let bridge_listener = tokio::net::TcpListener::bind(&cfg.bridge_listen)
        .await
        .map_err(|e| anyhow::anyhow!("bridge listen {}: {e}", cfg.bridge_listen))?;
    let bridge_local_addr = bridge_listener.local_addr()?;
    let bridge_endpoint = format!("http://{bridge_local_addr}");

    let shutdown_token = CancellationToken::new();
    let bridge_shutdown = shutdown_token.clone();
    let bridge_cascade = shutdown_token.clone();
    let bridge_ct_for_cascade = bridge_ct.clone();
    tokio::spawn(async move {
        bridge_cascade.cancelled().await;
        bridge_ct_for_cascade.cancel();
    });
    tokio::spawn(async move {
        if let Err(e) = axum::serve(bridge_listener, bridge_app)
            .with_graceful_shutdown(async move { bridge_shutdown.cancelled().await })
            .await
        {
            tracing::error!(err = %e, "embedded MCP bridge exited with error");
        }
    });
    tracing::info!(endpoint = %bridge_endpoint, "embedded MCP bridge listening");

    // 2. Construct AgentManager with the embedded MCP endpoint as override.
    let mut manager = AgentManager::new(
        cfg.store.clone(),
        cfg.agents_dir.clone(),
        event_bus.trace_sender(),
        event_bus.stream_sender(),
    );
    manager.set_bridge_endpoint_override(bridge_endpoint.clone());
    let manager = Arc::new(manager);

    // 3. Run the WS client loop. Reconnect on drop, capped backoff.
    let ws_loop = ws::run_ws_client_loop(cfg.clone(), manager.clone(), shutdown_token.clone());

    // 4. Graceful shutdown on Ctrl-C.
    let shutdown_token_ctrlc = shutdown_token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("bridge: shutting down...");
        shutdown_token_ctrlc.cancel();
    });

    ws_loop.await?;
    Ok(())
}
