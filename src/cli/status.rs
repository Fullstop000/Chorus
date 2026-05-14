//! `chorus status` — list channels, agents, and humans known to the server.
//!
//! Identity is resolved via `GET /api/whoami`; the resulting `humans.id` is
//! used to ask the server for that human's view of channels/agents/humans.
//! The CLI does not assume the OS user matches any Chorus identity.

pub async fn run(server_url: String) -> anyhow::Result<()> {
    let client = crate::utils::http::client();
    let (me, token) = crate::cli::fetch_authed_user_with_token(&client, &server_url).await?;
    let res = client
        .get(format!("{server_url}/internal/agent/{}/server", me.id))
        .bearer_auth(&token)
        .send()
        .await?;
    let data: serde_json::Value = res.json().await?;
    tracing::info!("== Local Human ==");
    tracing::info!("  @{} ({})", me.name, me.id);
    tracing::info!("== Channels ==");
    if let Some(channels) = data.get("channels").and_then(|v| v.as_array()) {
        for ch in channels {
            let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let joined = ch.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
            let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let status = if joined { "joined" } else { "not joined" };
            if desc.is_empty() {
                tracing::info!("  #{name} [{status}]");
            } else {
                tracing::info!("  #{name} [{status}] — {desc}");
            }
        }
    }
    if let Some(system_channels) = data.get("system_channels").and_then(|v| v.as_array()) {
        for ch in system_channels {
            let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if desc.is_empty() {
                tracing::info!("  #{name} [system]");
            } else {
                tracing::info!("  #{name} [system] — {desc}");
            }
        }
    }
    tracing::info!("\n== Agents ==");
    if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
        for a in agents {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            tracing::info!("  @{name} ({status})");
        }
    }
    tracing::info!("\n== Humans ==");
    if let Some(humans) = data.get("humans").and_then(|v| v.as_array()) {
        for h in humans {
            let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            tracing::info!("  @{name}");
        }
    }
    Ok(())
}
