//! `chorus channel list` — print channels known to the server.
//!
//! Default shows only channels the current user has joined; `--all` includes
//! every non-archived channel. `joined` is computed server-side from the
//! `member` query param, so we always pass the current user for an accurate
//! status column.

use anyhow::Context;

pub async fn run(all: bool, server_url: &str) -> anyhow::Result<()> {
    let username = whoami::username();
    // Always request system channels so the default path can surface rooms
    // like `#all` that every human is auto-joined to. The client-side filter
    // below drops rows with `joined=false` when `!all`, which keeps the
    // default output to "channels you've joined" as the help text promises.
    let url = format!(
        "{server_url}/api/channels?member={}&include_system=true",
        urlencoding::encode(&username)
    );
    let client = super::http::client();
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
        // Server's ?member= sets the `joined` flag per row but does not filter rows out.
        // Client filters here in the default path to honor "joined-only unless --all".
        if !all && !joined {
            continue;
        }
        let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
        // When `all` is false, every surviving row is joined — the tag is redundant.
        let tag = if all {
            if joined {
                "[joined]"
            } else {
                "[not joined]"
            }
        } else {
            ""
        };
        match (tag.is_empty(), desc.is_empty()) {
            (true, true) => tracing::info!("  #{name}"),
            (true, false) => tracing::info!("  #{name} — {desc}"),
            (false, true) => tracing::info!("  #{name} {tag}"),
            (false, false) => tracing::info!("  #{name} {tag} — {desc}"),
        }
        printed += 1;
    }
    if printed == 0 {
        tracing::info!("No channels.");
    }
    Ok(())
}
