//! `chorus status` — list all channels, agents, and humans known to the server.
//!
//! Calls `GET /internal/agent/<username>/server` and prints a formatted summary
//! of channels (with join status), agents (with runtime status), and humans.

pub async fn run(server_url: String) -> anyhow::Result<()> {
    let username = whoami::username();
    let client = chorus::utils::http::client();
    let res = client
        .get(format!("{server_url}/internal/agent/{username}/server"))
        .send()
        .await?;
    let data: serde_json::Value = res.json().await?;
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
