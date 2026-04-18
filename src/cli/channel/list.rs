//! `chorus channel list` — print channels known to the server.
//!
//! Default shows only channels the current user has joined; `--all` includes
//! every non-archived channel. `joined` is computed server-side from the
//! `member` query param, so we always pass the current user for an accurate
//! status column.

use anyhow::Context;

pub async fn run(all: bool, server_url: &str) -> anyhow::Result<()> {
    let username = whoami::username();
    let mut url = format!(
        "{server_url}/api/channels?member={}",
        urlencoding::encode(&username)
    );
    if all {
        // Include system channels (e.g. #all) so `--all` surfaces everything
        // visible, not just the default user/team set.
        url.push_str("&include_system=true");
    }
    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(super::surface_http_error(status, &body));
    }
    let data: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("unexpected response from {url}: not JSON"))?;
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected response from {url}: not a JSON array"))?;

    let mut printed = 0usize;
    for ch in arr {
        let joined = ch.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
        if !all && !joined {
            continue;
        }
        let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let label = if joined { "joined" } else { "not joined" };
        if desc.is_empty() {
            tracing::info!("  #{name} [{label}]");
        } else {
            tracing::info!("  #{name} [{label}] — {desc}");
        }
        printed += 1;
    }
    if printed == 0 {
        tracing::info!("No channels.");
    }
    Ok(())
}
