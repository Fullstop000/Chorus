//! `chorus start --no-open` (or `chorus serve`) — start the HTTP server and agent manager.
//!
//! Initialises the data directory layout, opens the SQLite store, spawns the
//! [`AgentManager`], loads agent templates, and starts the Axum HTTP server
//! on `0.0.0.0:<port>`. Agents lazy-start on the first incoming message;
//! no boot autorestart. Shuts down cleanly on Ctrl-C.
//!
//! Always starts the shared MCP HTTP bridge on `127.0.0.1:<bridge_port>` in
//! the same process. Both the main server and the bridge share a single
//! Ctrl-C handler via a `CancellationToken` — when the user hits Ctrl-C the
//! token is cancelled and both listeners drain gracefully.

use std::sync::Arc;

use chorus::agent::manager::AgentManager;
use chorus::server::event_bus::EventBus;
use chorus::store::Store;
use tokio_util::sync::CancellationToken;

use super::DATA_SUBDIR;

pub async fn run(
    port: u16,
    data_dir_str: String,
    template_dir_raw: String,
    bridge_port: u16,
) -> anyhow::Result<()> {
    let data_dir = std::path::PathBuf::from(&data_dir_str);
    let data_subdir = data_dir.join(DATA_SUBDIR);
    let logs_dir = data_dir.join("logs");
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&logs_dir)?;
    std::fs::create_dir_all(&agents_dir)?;
    let db_path = data_subdir.join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap())?);
    let event_bus = Arc::new(EventBus::new());

    // The local human (id + name) is resolved during `build_router_with_services`
    // from `ChorusConfig::local_human` or, on first run, by inserting a row keyed
    // by a freshly generated `human_<uuid>` id. Built-in channels and `#all`
    // membership are seeded against that id inside the router builder, so this
    // CLI no longer touches identity rows directly. `whoami::username()` is
    // intentionally absent here — request identity is `humans.id`, never the OS
    // user running the server process.

    let server_url = format!("http://localhost:{port}");
    let mut manager = AgentManager::new(store.clone(), agents_dir.clone(), event_bus.trace_sender(), event_bus.stream_sender());

    // Shared cancellation token — cancelled on Ctrl-C and used to shut down
    // both the main server and the bridge together.
    let shutdown_token = CancellationToken::new();

    // Bind the shared bridge before accepting HTTP traffic — agents require
    // a live bridge (no more stdio fallback). If the bridge port is taken
    // we fail to start.
    let bridge_listen = format!("127.0.0.1:{bridge_port}");
    let bridge_listener = tokio::net::TcpListener::bind(&bridge_listen)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "shared bridge: failed to bind {bridge_listen}: {e}. Is another chorus \
                 already running? Pass `--bridge-port` to use a different port."
            )
        })?;
    let bridge_local_addr = bridge_listener.local_addr().map_err(|e| {
        anyhow::anyhow!("shared bridge: failed to read local address for {bridge_listen}: {e}")
    })?;
    // Phase 1 bridge only supports loopback — guard in case the resolved
    // address is somehow non-loopback (shouldn't happen with 127.0.0.1, but
    // be defensive).
    if !bridge_local_addr.ip().is_loopback() {
        anyhow::bail!(
            "shared bridge: refusing non-loopback bind {bridge_local_addr}; bridge will not start"
        );
    }
    let actual_bridge_port = bridge_local_addr.port();

    // Same-process agents do not need the global discovery file: point this
    // server's agent manager at its co-hosted bridge directly so temp QA
    // instances can run alongside another Chorus without fighting over
    // ~/.chorus/bridge.json.
    manager.set_bridge_endpoint_override(format!("http://127.0.0.1:{actual_bridge_port}"));
    let manager = Arc::new(manager);

    // Write discovery info so external drivers can find the bridge.
    // A live discovery file owned by another Chorus is no longer fatal here:
    // this server's own agents use the explicit in-process bridge override
    // above, so isolated QA instances can coexist without stealing the global
    // discovery slot from the user's main server.
    let _discovery_guard = match chorus::bridge::discovery::write_bridge_info(
        &chorus::bridge::discovery::BridgeInfo {
            port: actual_bridge_port,
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
        },
    ) {
        Ok(()) => {
            // RAII guard removes the discovery file on every exit path —
            // normal shutdown, `?` propagation, or a panic during startup.
            // Without this, a panic between here and the bridge task
            // actually serving HTTP would leave a stale file that the
            // next `chorus serve` reads as "another chorus is alive,"
            // permanently blocking startup.
            Some(chorus::bridge::discovery::DiscoveryGuard)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            tracing::warn!(
                err = %e,
                port = actual_bridge_port,
                "shared bridge discovery file already owned by another chorus; continuing without publishing discovery"
            );
            None
        }
        Err(e) => {
            tracing::warn!(err = %e, "shared bridge: failed to write discovery file; bridge will still run");
            None
        }
    };

    tracing::info!(port = actual_bridge_port, "shared bridge listening");

    let (bridge_app, bridge_ct) = chorus::bridge::serve::build_bridge_router(&server_url);
    // Cascade the shared shutdown token into the bridge's internal CT so any
    // in-flight MCP sessions (child tokens spawned per request) drain when
    // Ctrl-C fires. Without this, axum stops accepting connections but active
    // sessions hang until their own timeouts.
    let bridge_shutdown = shutdown_token.clone();
    let bridge_cascade_trigger = shutdown_token.clone();
    let bridge_ct_for_cascade = bridge_ct.clone();
    tokio::spawn(async move {
        bridge_cascade_trigger.cancelled().await;
        bridge_ct_for_cascade.cancel();
    });
    tokio::spawn(async move {
        if let Err(e) = axum::serve(bridge_listener, bridge_app)
            .with_graceful_shutdown(async move { bridge_shutdown.cancelled().await })
            .await
        {
            tracing::error!(err = %e, "shared bridge exited with error");
        }
        // Note: the discovery file is removed by DiscoveryGuard held in the
        // outer scope on drop; don't remove it here or we double-up and
        // could race with a second serve that already stomp-checked.
        tracing::info!("shared bridge stopped");
    });

    // No boot autorestart: agents lazy-start on first incoming message
    // (see deliver_message_to_agents). The active agent_sessions row
    // (added in Phase 5) will ensure resume continuity when the next
    // start happens.

    // Load agent templates from the configured directory.
    let template_path = chorus::agent::templates::expand_tilde(&template_dir_raw);
    let templates = chorus::agent::templates::load_templates(&template_path);

    let router = chorus::server::build_router_with_services(
        store.clone(),
        event_bus.clone(),
        data_dir.clone(),
        agents_dir.clone(),
        manager.clone(),
        Arc::new(
            chorus::agent::runtime_status::SystemRuntimeStatusProvider::new(
                chorus::agent::manager::build_driver_registry(),
            ),
        ) as chorus::agent::runtime_status::SharedRuntimeStatusProvider,
        templates,
    );

    // Spawn background trace writer for Telescope persistence.
    chorus::store::trace_writer::spawn_trace_writer(
        db_path.to_str().unwrap().to_string(),
        event_bus.subscribe_traces(),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Chorus running at {server_url}");
    tracing::info!("Use `chorus send '#all' 'hello'` to send messages");
    tracing::info!("Use `chorus agent create <name>` to create an agent");

    // Graceful shutdown on Ctrl+C — cancel the shared token so both the main
    // server and the co-hosted bridge drain together.
    let shutdown_token_ctrlc = shutdown_token.clone();
    let shutdown = async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("\nShutting down...");
        shutdown_token_ctrlc.cancel();
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
