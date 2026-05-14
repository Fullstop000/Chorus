//! `chorus send <target> <content>` — post a message as the local human.
//!
//! Resolves identity via `GET /api/whoami` (the server-resolved
//! `humans.id`/`humans.name`), then dispatches to the public conversation
//! send endpoint. The CLI no longer infers identity from `whoami::username()`
//! — the OS user running the CLI is not the Chorus human row.

use anyhow::Context;

pub async fn run(target: String, content: String, server_url: String) -> anyhow::Result<()> {
    let client = crate::utils::http::client();
    let (me, token) = crate::cli::fetch_authed_user_with_token(&client, &server_url).await?;

    // The historical `/internal/agent/{actor_id}/send` route is keyed on
    // sender id and works for either humans or agents — `handle_send`
    // resolves the actor type from the id rather than the route. Sending
    // here uses the local human's id so the resulting row's
    // `(sender_id, sender_type)` is `(human.id, "human")`. The bridge_auth
    // layer permits CLI tokens on /internal/agent/{user.id}/* because the
    // actor id matches the token's user_id.
    let res = client
        .post(format!("{server_url}/internal/agent/{}/send", me.id))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "target": target, "content": content }))
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let data: serde_json::Value = res.json().await?;
    if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
        return Err(crate::cli::CliError(err.to_string()).into());
    }
    let msg_id = data
        .get("messageId")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    tracing::info!("Message sent to {target} as @{}. ID: {msg_id}", me.name);
    Ok(())
}
