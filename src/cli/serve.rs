//! `chorus start --no-open` (or `chorus serve`) — start the HTTP server and agent manager.
//!
//! Initialises the data directory layout, opens the SQLite store, spawns the
//! [`AgentManager`] (which auto-restarts previously-active agents), loads
//! agent templates, and starts the Axum HTTP server on `0.0.0.0:<port>`.
//! Shuts down cleanly on Ctrl-C.
//!
//! Always starts the shared MCP HTTP bridge on `127.0.0.1:<bridge_port>` in
//! the same process. Both the main server and the bridge share a single
//! Ctrl-C handler via a `CancellationToken` — when the user hits Ctrl-C the
//! token is cancelled and both listeners drain gracefully.

use std::sync::Arc;

use chorus::agent::manager::AgentManager;
use chorus::store::agents::AgentStatus;
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
    let store =
        Arc::new(Store::open(db_path.to_str().unwrap())?.with_agents_dir(agents_dir.clone()));

    // Default human = OS username
    let username = whoami::username();
    let _ = store.create_human(&username);

    // Ensure built-in system channels exist and upgrade legacy installs to #all.
    store.ensure_builtin_channels(&username)?;

    let server_url = format!("http://localhost:{port}");
    let manager = Arc::new(AgentManager::new(store.clone(), agents_dir));

    // Shared cancellation token — cancelled on Ctrl-C and used to shut down
    // both the main server and the bridge together.
    let shutdown_token = CancellationToken::new();

    // Bind the shared bridge BEFORE auto-restarting agents — agents now
    // require a live bridge (no more stdio fallback). If the bridge port is
    // taken we fail to start.
    let bridge_listen = format!("127.0.0.1:{bridge_port}");
    let bridge_listener = tokio::net::TcpListener::bind(&bridge_listen)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "shared bridge: failed to bind {bridge_listen}: {e}. Is another chorus \
                 already running? Pass `--bridge-port` to use a different port."
            )
        })?;
    let bridge_local_addr = bridge_listener
        .local_addr()
        .expect("bridge listener has a local addr");
    // Phase 1 bridge only supports loopback — guard in case the resolved
    // address is somehow non-loopback (shouldn't happen with 127.0.0.1, but
    // be defensive).
    if !bridge_local_addr.ip().is_loopback() {
        anyhow::bail!(
            "shared bridge: refusing non-loopback bind {bridge_local_addr}; bridge will not start"
        );
    }
    let actual_bridge_port = bridge_local_addr.port();

    // Write discovery info so drivers can find the bridge.
    // `AlreadyExists` means another live `chorus serve` owns the discovery
    // file — abort hard so we don't silently steal its agents' routing.
    // Other errors (permissions, disk) warn-and-continue so the bridge still
    // runs for same-process agents.
    match chorus::bridge::discovery::write_bridge_info(&chorus::bridge::discovery::BridgeInfo {
        port: actual_bridge_port,
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    }) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!(
                "shared bridge: {e}. Stop the other chorus server (or wait for it to exit) \
                 before starting a new one."
            );
        }
        Err(e) => {
            tracing::warn!(err = %e, "shared bridge: failed to write discovery file; bridge will still run");
        }
    }

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
        // Clean up discovery file when bridge shuts down.
        chorus::bridge::discovery::remove_bridge_info();
        tracing::info!("shared bridge stopped");
    });

    // Auto-restart agents that were active before server restart.
    // Track failures per agent so repeated failures can be surfaced.
    {
        let active_agents: Vec<String> = store
            .get_agents()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.status == AgentStatus::Active)
            .map(|a| a.name)
            .collect();
        let mut failed_agents = Vec::new();
        for agent_name in active_agents {
            tracing::info!(agent = %agent_name, "auto-restarting active agent");
            if let Err(e) = manager.start_agent(&agent_name, None).await {
                let error_detail = format!("{e:#}");
                tracing::error!(agent = %agent_name, err = %error_detail, "failed to restart agent — marking inactive so subsequent delivery can retry");
                // Mark inactive so next message delivery can attempt a fresh start
                if let Err(e) = store.update_agent_status(&agent_name, AgentStatus::Inactive) {
                    tracing::error!(agent = %agent_name, err = %e, "also failed to mark agent inactive — manual intervention required");
                }
                failed_agents.push((agent_name, error_detail));
            }
        }
        if !failed_agents.is_empty() {
            tracing::warn!(
                "Warning: {} agent(s) failed to auto-restart and were marked inactive: {}",
                failed_agents.len(),
                failed_agents
                    .iter()
                    .map(|(agent_name, _)| agent_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for (agent_name, error_detail) in &failed_agents {
                tracing::warn!("  - {agent_name}: {error_detail}");
            }
            tracing::warn!("They will be retried on next message delivery. To restart immediately: `chorus agent start <name>`");
        }
    }

    // Load agent templates from the configured directory.
    let template_path = chorus::agent::templates::expand_tilde(&template_dir_raw);
    let templates = chorus::agent::templates::load_templates(&template_path);

    let router = chorus::server::build_router_with_services(
        store.clone(),
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
        store.subscribe_traces(),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Chorus running at {server_url}");
    tracing::info!("Human user: @{username}");
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
