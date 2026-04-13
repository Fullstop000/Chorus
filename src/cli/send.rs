//! `chorus send <target> <content>` — post a message as the OS user.
//!
//! Calls `POST /internal/agent/<username>/send` on the running server.
//! `<target>` is a channel (`#general`) or DM handle (`dm:@alice`).

pub async fn run(target: String, content: String, server_url: String) -> anyhow::Result<()> {
    let username = whoami::username();
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{server_url}/internal/agent/{username}/send"))
        .json(&serde_json::json!({ "target": target, "content": content }))
        .send()
        .await?;
    let data: serde_json::Value = res.json().await?;
    if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
        tracing::error!("Error: {err}");
    } else {
        let msg_id = data
            .get("messageId")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        tracing::info!("Message sent to {target}. ID: {msg_id}");
    }
    Ok(())
}
