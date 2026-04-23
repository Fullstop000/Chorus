//! `chorus channel …` — subcommand group for channel management.
//!
//! All subcommands talk to the running Chorus server over HTTP. The parent
//! `channel` command owns `--server-url` as a global arg so each subcommand
//! inherits it without redeclaring the flag.

mod create;
mod delete;
mod history;
mod join;
mod list;

use anyhow::Context;
use clap::Subcommand;

pub(super) use chorus::store::channels::normalize_channel_name;
pub(super) use chorus::utils::http;

/// Upper bound on `channel history --limit`. The server streams rows from
/// SQLite without a cap of its own; clamp here so a typo like `--limit 99999`
/// can't spike memory, and a negative value can't become SQLite's "no limit"
/// sentinel (`LIMIT -1`).
const HISTORY_LIMIT_MAX: i64 = 500;

#[derive(Subcommand)]
pub(crate) enum ChannelCommands {
    /// Create a new channel
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a channel
    #[command(visible_alias = "delete")]
    Del {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Join a channel as the current OS user
    Join { name: String },
    /// List channels (default: only channels you've joined)
    List {
        #[arg(long)]
        all: bool,
    },
    /// Print recent messages from a channel
    History {
        name: String,
        #[arg(long, default_value = "20", value_parser = clap::value_parser!(i64).range(1..=HISTORY_LIMIT_MAX))]
        limit: i64,
    },
}

pub async fn run(server_url: String, cmd: ChannelCommands) -> anyhow::Result<()> {
    match cmd {
        ChannelCommands::Create { name, description } => {
            create::run(name, description, &server_url).await
        }
        ChannelCommands::Del { name, yes } => delete::run(name, yes, &server_url).await,
        ChannelCommands::Join { name } => join::run(name, &server_url).await,
        ChannelCommands::List { all } => list::run(all, &server_url).await,
        ChannelCommands::History { name, limit } => history::run(name, limit, &server_url).await,
    }
}

/// Turn a non-2xx HTTP response into an `anyhow::Error`.
///
/// Parses the server's `ErrorResponse` JSON (`{error, code?}`). When a typed
/// `code` is present, surfaces it as `<code>: <error>`. Falls back to status +
/// raw body when the body isn't the expected shape.
pub(crate) fn surface_http_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        let msg = val.get("error").and_then(|v| v.as_str()).unwrap_or("");
        let code = val.get("code").and_then(|v| v.as_str());
        if let Some(code) = code {
            return anyhow::anyhow!("{}: {}", code.to_lowercase(), msg);
        }
        if !msg.is_empty() {
            return anyhow::anyhow!("{status}: {msg}");
        }
    }
    anyhow::anyhow!("{status}: {body}")
}

/// Resolve a channel name (with or without a leading `#`) to its id by listing
/// all channels from the server and matching on the normalized name.
pub(super) async fn resolve_channel_id(
    client: &reqwest::Client,
    server_url: &str,
    name: &str,
) -> anyhow::Result<String> {
    let normalized = normalize_channel_name(name);
    let url = format!("{server_url}/api/channels");
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
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected response from {url}: not a JSON array"))?;
    for ch in arr {
        let ch_name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if ch_name == normalized {
            let id = ch
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("channel entry missing id"))?;
            return Ok(id.to_string());
        }
    }
    anyhow::bail!("channel not found: #{normalized}")
}
