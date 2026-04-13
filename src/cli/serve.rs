//! `chorus serve` — start the HTTP server and agent manager.
//!
//! Initialises the data directory layout, opens the SQLite store, spawns the
//! [`AgentManager`] (which auto-restarts previously-active agents), loads
//! agent templates, and starts the Axum HTTP server on `0.0.0.0:<port>`.
//! Shuts down cleanly on Ctrl-C.

use std::sync::Arc;

use chorus::agent::manager::AgentManager;
use chorus::store::agents::AgentStatus;
use chorus::store::Store;

use super::DATA_SUBDIR;

pub async fn run(port: u16, data_dir_str: String, template_dir_raw: String) -> anyhow::Result<()> {
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
    let bridge_binary = std::env::current_exe()?.to_string_lossy().into_owned();
    let manager = Arc::new(AgentManager::new(
        store.clone(),
        agents_dir,
        bridge_binary,
        server_url.clone(),
    ));

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
        Arc::new(chorus::agent::runtime_status::SystemRuntimeStatusProvider)
            as chorus::agent::runtime_status::SharedRuntimeStatusProvider,
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

    // Graceful shutdown on Ctrl+C
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("\nShutting down...");
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
