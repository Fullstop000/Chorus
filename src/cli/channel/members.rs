//! `chorus channel members <name>` — list members of a channel.

use anyhow::Context;

pub async fn run(name: String, server_url: &str) -> anyhow::Result<()> {
    let client = super::http::client();
    let token = crate::cli::resolve_cli_token()?;
    let normalized = super::normalize_channel_name(&name);
    let channel_id = super::resolve_channel_id(&client, server_url, &normalized, &token).await?;

    let url = format!("{server_url}/api/channels/{channel_id}/members");
    let res = client
        .get(&url)
        .bearer_auth(&token)
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
    let member_count = data
        .get("memberCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    tracing::info!(
        "{member_count} member{} in #{normalized}:",
        if member_count == 1 { "" } else { "s" }
    );
    if let Some(members) = data.get("members").and_then(|v| v.as_array()) {
        for m in members {
            let member_name = m.get("memberName").and_then(|v| v.as_str()).unwrap_or("?");
            let member_type = m
                .get("memberType")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let display = m
                .get("displayName")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            match display {
                Some(d) => tracing::info!("  @{member_name} ({member_type}) — {d}"),
                None => tracing::info!("  @{member_name} ({member_type})"),
            }
        }
    }
    Ok(())
}
