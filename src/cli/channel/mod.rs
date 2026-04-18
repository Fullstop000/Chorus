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

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum ChannelCommands {
    /// Create a new channel
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a channel
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
        #[arg(long, default_value = "20")]
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

/// Resolve a channel name (with or without a leading `#`) to its id by listing
/// all channels from the server and matching on the normalized name.
///
/// Mirrors `server::handlers::channels::normalize_channel_name`: trim, strip a
/// single leading `#`, trim again, lowercase.
#[allow(dead_code)] // used by future subcommands (del, join) landing in later tasks
pub(super) async fn resolve_channel_id(
    client: &reqwest::Client,
    server_url: &str,
    name: &str,
) -> anyhow::Result<String> {
    let normalized = name
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_lowercase();
    let url = format!("{server_url}/api/channels");
    let res = client.get(&url).send().await.map_err(|e| {
        anyhow::anyhow!(
            "{e}: is the Chorus server running at {server_url}?"
        )
    })?;
    let data: serde_json::Value = res.json().await?;
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
