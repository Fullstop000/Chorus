//! `chorus agent <subcommand>` — manage agents.
//!
//! Subcommands:
//! - `create <name>` — register a new agent record in the store and join it to
//!   all existing channels. The agent is picked up on the next `chorus start`.
//! - `stop <name>`   — mark an agent inactive so the manager stops it on the
//!   next heartbeat (or immediately if the server is running).
//! - `list`          — list all agents with their status and runtime.

use chorus::store::agents::AgentStatus;
use chorus::store::messages::SenderType;
use chorus::store::{AgentRecordUpsert, Store};

use super::{db_path_for, default_data_dir, default_model_for_runtime, AgentCommands};

pub async fn run(cmd: AgentCommands) -> anyhow::Result<()> {
    match cmd {
        AgentCommands::Create {
            name,
            runtime,
            model,
            description,
            data_dir,
        } => {
            let model = if model.is_empty() {
                default_model_for_runtime(&runtime).to_string()
            } else {
                model
            };
            let data_dir = data_dir.unwrap_or_else(default_data_dir);
            let db_path = db_path_for(&data_dir);
            let store = Store::open(&db_path)?;
            store.create_agent_record(&AgentRecordUpsert {
                name: &name,
                display_name: &name,
                description: description.as_deref(),
                system_prompt: None,
                runtime: &runtime,
                model: &model,
                reasoning_effort: None,
                env_vars: &[],
            })?;
            // Join default channels
            for ch in store.get_channels()? {
                let _ = store.join_channel(&ch.name, &name, SenderType::Agent);
            }
            tracing::info!("Agent @{name} created (runtime: {runtime}, model: {model}).");
            tracing::info!("Start it by running the server: `chorus start`");
            Ok(())
        }
        AgentCommands::Stop { name, data_dir } => {
            tracing::info!("Stopping agent @{name}...");
            let data_dir = data_dir.unwrap_or_else(default_data_dir);
            let db_path = db_path_for(&data_dir);
            let store = Store::open(&db_path)?;
            store.update_agent_status(&name, AgentStatus::Inactive)?;
            tracing::info!("Agent @{name} marked as inactive.");
            Ok(())
        }
        AgentCommands::List { server_url } => {
            let client = reqwest::Client::new();
            let username = whoami::username();
            let res = client
                .get(format!("{server_url}/internal/agent/{username}/server"))
                .send()
                .await?;
            let data: serde_json::Value = res.json().await?;
            if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
                if agents.is_empty() {
                    tracing::info!("No agents.");
                } else {
                    for a in agents {
                        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let runtime = a.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
                        tracing::info!("  @{name} [{status}] (runtime: {runtime})");
                    }
                }
            }
            Ok(())
        }
    }
}
