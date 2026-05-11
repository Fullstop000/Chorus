//! `chorus channel create <name>` — POST a new channel to the server.
//!
//! Calls `POST /api/channels` with `{ name, description }`. The name is
//! normalized client-side so the success line matches what the server stored.

use anyhow::Context;

pub async fn run(
    name: String,
    description: Option<String>,
    server_url: &str,
) -> anyhow::Result<()> {
    let normalized = super::normalize_channel_name(&name);
    if !chorus::store::channels::is_valid_channel_name(&normalized) {
        return Err(crate::cli::CliError(format!(
            "{}: {normalized}",
            chorus::store::channels::INVALID_CHANNEL_NAME_MSG
        ))
        .into());
    }
    let description = description.unwrap_or_default();
    let client = super::http::client();
    let url = format!("{server_url}/api/channels");
    let res = client
        .post(&url)
        .json(&serde_json::json!({
            "name": normalized,
            "description": description,
        }))
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    if status.is_success() {
        tracing::info!("Channel #{normalized} created.");
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    Err(super::surface_http_error(status, &body))
}
