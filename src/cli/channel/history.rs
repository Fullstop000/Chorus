//! `chorus channel history <name>` — print recent messages from a channel.
//!
//! Calls `GET /internal/agent/<username>/history?channel=<name>&limit=<limit>`
//! and prints each message as `[<timestamp>] @<sender>: <content>`.

pub async fn run(name: String, limit: i64, server_url: &str) -> anyhow::Result<()> {
    let username = whoami::username();
    let client = reqwest::Client::new();
    let res = client
        .get(format!(
            "{server_url}/internal/agent/{username}/history?channel={}&limit={limit}",
            urlencoding::encode(&name)
        ))
        .send()
        .await?;
    let data: serde_json::Value = res.json().await?;
    if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
        tracing::error!("Error: {err}");
    } else if let Some(messages) = data.get("messages").and_then(|v| v.as_array()) {
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
    }
    Ok(())
}
