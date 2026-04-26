//! `chorus channel join <name>` — add the local human to a channel.
//!
//! Resolves the channel id via the shared helper, then invites the local
//! human (server-resolved `humans.id`/`humans.name` from `/api/whoami`).
//! There is no dedicated self-join endpoint; we piggyback on the invite path
//! because the server treats self-invites as idempotent joins.

use anyhow::Context;

pub async fn run(name: String, server_url: &str) -> anyhow::Result<()> {
    let client = super::http::client();
    let me = crate::cli::fetch_local_human_identity(&client, server_url).await?;
    let normalized = super::normalize_channel_name(&name);
    let channel_id = super::resolve_channel_id(&client, server_url, &normalized).await?;

    // Piggybacks on the invite endpoint because there is no dedicated self-join
    // route; server-side self-invites are idempotent joins. The invite handler
    // resolves a name OR id via `lookup_sender_ref`, so passing `humans.id`
    // here keeps the round-trip stable even if the human's name changes.
    let url = format!("{server_url}/api/channels/{channel_id}/members");
    let res = client
        .post(&url)
        .json(&serde_json::json!({ "memberName": me.id }))
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    if status.is_success() {
        tracing::info!("Joined #{normalized} as @{}.", me.name);
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    Err(super::surface_http_error(status, &body))
}
