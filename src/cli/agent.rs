//! `chorus agent <subcommand>` — manage agents.
//!
//! Subcommands:
//! - `create <name>` — POST to the running server's `/api/agents` endpoint.
//! - `stop <name>`   — mark an agent inactive so the manager stops it on the
//!   next heartbeat (or immediately if the server is running).
//! - `list`          — list all agents with their status and runtime.

use std::io::{BufRead, IsTerminal, Write};

use super::{default_model_for_runtime, AgentCommands};
use anyhow::Context;

fn resolve_agent_id(agents: &[serde_json::Value], name: &str) -> anyhow::Result<String> {
    // First: exact match on canonical name (strongest identity).
    if let Some(id) = agents.iter().find_map(|agent| {
        (agent.get("name").and_then(|v| v.as_str()) == Some(name))
            .then(|| agent.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .flatten()
    }) {
        return Ok(id);
    }

    // Second: fallback to display_name, but reject ambiguity.
    let display_matches: Vec<String> = agents
        .iter()
        .filter(|agent| agent.get("display_name").and_then(|v| v.as_str()) == Some(name))
        .filter_map(|agent| agent.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();

    match display_matches.len() {
        0 => Err(crate::cli::UserError(format!("agent not found: {name}")).into()),
        1 => Ok(display_matches.into_iter().next().unwrap()),
        _ => Err(crate::cli::UserError(format!(
            "ambiguous name: '{name}' matches {} agents by display_name. Use the canonical name (e.g., testbot-2a00).",
            display_matches.len()
        )).into()),
    }
}

async fn fetch_agent_list(
    client: &reqwest::Client,
    server_url: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let res = client
        .get(format!("{server_url}/api/agents"))
        .send()
        .await?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "server returned {status} while listing agents: {}",
            super::channel::surface_http_error(status, &body)
        );
    }
    let agents: Vec<serde_json::Value> = serde_json::from_str(&body)
        .with_context(|| format!("unexpected agent list response from {server_url}"))?;
    Ok(agents)
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
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
            }
            let data: serde_json::Value = serde_json::from_str(&body)
                .with_context(|| format!("unexpected create response from {server_url}"))?;
            let agent_name = data.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            tracing::info!("Agent @{agent_name} created (runtime: {runtime}, model: {model}).");
            Ok(())
        }
        AgentCommands::Stop { name, server_url } => {
            tracing::info!("Stopping agent @{name}...");
            let client = chorus::utils::http::client();
            let agents = fetch_agent_list(&client, &server_url).await?;
            let agent_id = resolve_agent_id(&agents, &name)?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/stop"))
                .send()
                .await?;
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
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
            let agents = fetch_agent_list(&client, &server_url).await?;
            let agent_id = resolve_agent_id(&agents, &name)?;
            let res = client
                .get(format!("{server_url}/api/agents/{agent_id}"))
                .send()
                .await?;
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
            }
            let data: serde_json::Value = serde_json::from_str(&body)
                .with_context(|| format!("unexpected get response from {server_url}"))?;
            if let Some(agent) = data.get("agent") {
                let id = agent.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let agent_name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let runtime = agent.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
                let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                let status = agent.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let description = agent
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                tracing::info!("Agent @{agent_name}");
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
            let agents = fetch_agent_list(&client, &server_url).await?;
            let agent_id = resolve_agent_id(&agents, &name)?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/start"))
                .send()
                .await?;
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
            }
            tracing::info!("Agent @{name} started.");
            Ok(())
        }
        AgentCommands::Restart {
            name,
            mode,
            server_url,
        } => {
            tracing::info!("Restarting agent @{name} (mode: {mode})...");
            let client = chorus::utils::http::client();
            let agents = fetch_agent_list(&client, &server_url).await?;
            let agent_id = resolve_agent_id(&agents, &name)?;
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/restart"))
                .json(&serde_json::json!({ "mode": mode.as_str() }))
                .send()
                .await?;
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
            }
            tracing::info!("Agent @{name} restarted.");
            Ok(())
        }
        AgentCommands::Delete {
            name,
            wipe,
            yes,
            server_url,
        } => {
            if !yes {
                let display_name = if wipe {
                    format!("{name} (with --wipe)")
                } else {
                    name.clone()
                };
                eprint!("Delete agent @{display_name}? [y/N] ");
                std::io::stderr().flush().ok();
                let stdin = std::io::stdin();
                let is_tty = stdin.is_terminal();
                let mut locked = stdin.lock();
                let mut line = String::new();
                if !is_tty {
                    return Err(crate::cli::UserError(format!(
                        "refusing to delete @{name} without --yes on non-interactive stdin"
                    ))
                    .into());
                }
                if locked.read_line(&mut line).is_err() {
                    return Err(crate::cli::UserError("Abort.".into()).into());
                }
                let trimmed = line.trim();
                if !matches!(trimmed, "y" | "Y") {
                    tracing::info!("Aborted.");
                    return Ok(());
                }
            }

            tracing::info!("Deleting agent @{name}...");
            let client = chorus::utils::http::client();
            let agents = fetch_agent_list(&client, &server_url).await?;
            let agent_id = resolve_agent_id(&agents, &name)?;
            let mode = if wipe {
                "delete_workspace"
            } else {
                "preserve_workspace"
            };
            let res = client
                .post(format!("{server_url}/api/agents/{agent_id}/delete"))
                .json(&serde_json::json!({ "mode": mode }))
                .send()
                .await?;
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "server returned {status}: {}",
                    super::channel::surface_http_error(status, &body)
                );
            }
            let data: serde_json::Value = serde_json::from_str(&body)
                .with_context(|| format!("unexpected delete response from {server_url}"))?;
            if let Some(warning) = data.get("warning").and_then(|v| v.as_str()) {
                if wipe {
                    return Err(crate::cli::UserError(format!(
                        "Agent deleted but workspace cleanup failed: {warning}"
                    ))
                    .into());
                }
                tracing::warn!("{warning}");
            }
            tracing::info!("Agent @{name} deleted.");
            Ok(())
        }
    }
}
