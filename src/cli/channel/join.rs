//! `chorus channel join <name>` — add the current OS user to a channel.
//!
//! Resolves the channel id via the shared helper, then invites the current
//! user. There is no dedicated self-join endpoint; we piggyback on the invite
//! path because the server treats self-invites as idempotent joins.

use anyhow::Context;

pub async fn run(name: String, server_url: &str) -> anyhow::Result<()> {
    let username = whoami::username();
    let normalized = super::normalize_channel_name(&name);
    let client = reqwest::Client::new();
    let channel_id = super::resolve_channel_id(&client, server_url, &name).await?;

    // Piggybacks on the invite endpoint because there is no dedicated self-join
    // route; server-side self-invites are idempotent joins.
    let url = format!("{server_url}/api/channels/{channel_id}/members");
    let res = client
        .post(&url)
        .json(&serde_json::json!({ "memberName": username }))
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    if status.is_success() {
        tracing::info!("Joined #{normalized} as @{username}.");
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    Err(super::surface_http_error(status, &body))
}
