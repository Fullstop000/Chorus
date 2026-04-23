//! `chorus agent <subcommand>` — manage agents.
//!
//! Subcommands:
//! - `create <name>` — POST to the running server's `/api/agents` endpoint.
//! - `stop <name>`   — mark an agent inactive so the manager stops it on the
//!   next heartbeat (or immediately if the server is running).
//! - `list`          — list all agents with their status and runtime.

use super::{default_model_for_runtime, AgentCommands};

fn find_agent_id_by_name(agents: &[serde_json::Value], name: &str) -> Option<String> {
    agents.iter().find_map(|agent| {
        let matches = agent.get("name").and_then(|v| v.as_str()) == Some(name)
            || agent.get("display_name").and_then(|v| v.as_str()) == Some(name);
        matches
            .then(|| agent.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .flatten()
    })
}

pub async fn run(cmd: AgentCommands) -> anyhow::Result<()> {
    match cmd {
        AgentCommands::Create {
            name,
            runtime,
            model,
            description,
            server_url,
        } => {
            let model = if model.is_empty() {
                default_model_for_runtime(&runtime).to_string()
            } else {
                model
            };
            let client = chorus::utils::http::client();
            let mut payload = serde_json::json!({
                "display_name": name,
                "runtime": runtime,
                "model": model,
            });
            if let Some(desc) = description {
                payload["description"] = serde_json::json!(desc);
            }
            let res = client
                .post(format!("{server_url}/api/agents"))
                .json(&payload)
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            let data: serde_json::Value = res.json().await?;
            let agent_name = data.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            tracing::info!("Agent @{agent_name} created (runtime: {runtime}, model: {model}).");
            Ok(())
        }
        AgentCommands::Stop { name, server_url } => {
            tracing::info!("Stopping agent @{name}...");
            let client = chorus::utils::http::client();
            let list_res = client
                .get(format!("{server_url}/api/agents"))
                .send()
                .await?;
            let list_status = list_res.status();
            if !list_status.is_success() {
                let body = list_res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {list_status} while listing agents: {body}");
            }
            let agents: Vec<serde_json::Value> = list_res.json().await?;
            let agent_id = find_agent_id_by_name(&agents, &name)
                .ok_or_else(|| anyhow::anyhow!("agent not found: {name}"))?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/stop"))
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            tracing::info!("Agent @{name} stopped.");
            Ok(())
        }
        AgentCommands::List { server_url } => {
            let client = chorus::utils::http::client();
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
        AgentCommands::Get { name, server_url } => {
            let client = chorus::utils::http::client();
            let list_res = client
                .get(format!("{server_url}/api/agents"))
                .send()
                .await?;
            let list_status = list_res.status();
            if !list_status.is_success() {
                let body = list_res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {list_status} while listing agents: {body}");
            }
            let agents: Vec<serde_json::Value> = list_res.json().await?;
            let agent_id = find_agent_id_by_name(&agents, &name)
                .ok_or_else(|| anyhow::anyhow!("agent not found: {name}"))?;
            let res = client
                .get(format!("{server_url}/api/agents/{agent_id}"))
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            let data: serde_json::Value = res.json().await?;
            if let Some(agent) = data.get("agent") {
                let id = agent.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let runtime = agent.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
                let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                let status = agent.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let description = agent
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                tracing::info!("Agent @{name}");
                tracing::info!("  id:          {id}");
                tracing::info!("  runtime:     {runtime}");
                tracing::info!("  model:       {model}");
                tracing::info!("  status:      {status}");
                if !description.is_empty() {
                    tracing::info!("  description: {description}");
                }
            } else {
                anyhow::bail!("server response missing agent field");
            }
            Ok(())
        }
        AgentCommands::Start { name, server_url } => {
            tracing::info!("Starting agent @{name}...");
            let client = chorus::utils::http::client();
            let list_res = client
                .get(format!("{server_url}/api/agents"))
                .send()
                .await?;
            let list_status = list_res.status();
            if !list_status.is_success() {
                let body = list_res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {list_status} while listing agents: {body}");
            }
            let agents: Vec<serde_json::Value> = list_res.json().await?;
            let agent_id = find_agent_id_by_name(&agents, &name)
                .ok_or_else(|| anyhow::anyhow!("agent not found: {name}"))?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/start"))
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            tracing::info!("Agent @{name} started.");
            Ok(())
        }
        AgentCommands::Restart { name, mode, server_url } => {
            tracing::info!("Restarting agent @{name} (mode: {mode})...");
            let client = chorus::utils::http::client();
            let list_res = client
                .get(format!("{server_url}/api/agents"))
                .send()
                .await?;
            let list_status = list_res.status();
            if !list_status.is_success() {
                let body = list_res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {list_status} while listing agents: {body}");
            }
            let agents: Vec<serde_json::Value> = list_res.json().await?;
            let agent_id = find_agent_id_by_name(&agents, &name)
                .ok_or_else(|| anyhow::anyhow!("agent not found: {name}"))?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/restart"))
                .json(&serde_json::json!({ "mode": mode }))
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            tracing::info!("Agent @{name} restarted.");
            Ok(())
        }
        AgentCommands::Delete { name, wipe, server_url } => {
            tracing::info!("Deleting agent @{name}...");
            let client = chorus::utils::http::client();
            let list_res = client
                .get(format!("{server_url}/api/agents"))
                .send()
                .await?;
            let list_status = list_res.status();
            if !list_status.is_success() {
                let body = list_res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {list_status} while listing agents: {body}");
            }
            let agents: Vec<serde_json::Value> = list_res.json().await?;
            let agent_id = find_agent_id_by_name(&agents, &name)
                .ok_or_else(|| anyhow::anyhow!("agent not found: {name}"))?;
            let mode = if wipe { "delete_workspace" } else { "preserve_workspace" };
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/delete"))
                .json(&serde_json::json!({ "mode": mode }))
                .send()
                .await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                anyhow::bail!("server returned {status}: {body}");
            }
            let data: serde_json::Value = res.json().await?;
            if let Some(warning) = data.get("warning").and_then(|v| v.as_str()) {
                tracing::warn!("{warning}");
            }
            tracing::info!("Agent @{name} deleted.");
            Ok(())
        }
    }
}
