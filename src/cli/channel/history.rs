//! `chorus channel history <name>` — print recent messages from a channel.
//!
//! Calls `GET /internal/agent/<username>/history?channel=<name>&limit=<limit>`
//! and prints each message as `[<timestamp>] @<sender>: <content>`.

use anyhow::Context;

use super::surface_http_error;

pub async fn run(name: String, limit: i64, server_url: &str) -> anyhow::Result<()> {
    let username = whoami::username();
    let client = super::http::client();
    // Normalize the user input (trim, strip leading `#`, lowercase) and then
    // send it back with a leading `#`. The server's `resolve_history_target`
    // only runs its own normalization on targets that already start with `#`
    // or `dm:@`; a bare `General` would otherwise be looked up literally.
    let channel_target = format!("#{}", super::normalize_channel_name(&name));
    let url = format!(
        "{server_url}/internal/agent/{username}/history?channel={}&limit={limit}",
        urlencoding::encode(&channel_target)
    );
    let res = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(surface_http_error(status, &body));
    }
    let data: serde_json::Value =
        serde_json::from_str(&body).with_context(|| format!("unexpected response from {url}"))?;
    if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("{err}");
    }
    let messages = data
        .get("messages")
        .and_then(|v| v.as_array())
        .context("missing `messages` field in history response")?;
    if messages.is_empty() {
        tracing::info!("No messages.");
    } else {
        for m in messages {
            let sender = m.get("senderName").and_then(|v| v.as_str()).unwrap_or("?");
            let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let time = m.get("createdAt").and_then(|v| v.as_str()).unwrap_or("");
            tracing::info!("[{time}] @{sender}: {content}");
        }
    }
    Ok(())
}
