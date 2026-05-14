//! Shared CLI module: submodule declarations + helpers used by the
//! `platform` and `bridge` binaries (see `src/bin/`).
//!
//! Each subcommand is implemented as its own submodule whose `run`
//! function does the actual work. The bin entry points (`src/bin/platform.rs`
//! and `src/bin/bridge.rs`) own the clap `Cli` struct and dispatch into
//! these submodules.

pub mod agent;
pub mod bridge;
pub mod channel;
pub mod check;
pub mod credentials;
pub mod login;
pub mod logout;
pub mod send;
pub mod serve;
pub mod setup;
pub mod start;
pub mod status;
pub mod workspace;

use clap::{Subcommand, ValueEnum};

use crate::agent::AgentRuntime;
use crate::config::ChorusConfig;

/// An expected user error that should be printed cleanly without a backtrace.
/// The CLI enables `RUST_BACKTRACE=1` by default, so `anyhow::bail!`
/// would emit a full backtrace for mundane mistakes like forgetting `--yes`.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Create a new agent via the running server
    Create {
        name: String,
        #[arg(long, default_value = "claude")]
        runtime: String,
        #[arg(long, default_value = "")]
        model: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Stop a running agent
    Stop {
        name: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// List all agents
    List {
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Show agent details
    Get {
        name: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Start a sleeping agent
    Start {
        name: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Restart an agent
    Restart {
        name: String,
        #[arg(long, default_value_t = RestartMode::Restart)]
        mode: RestartMode,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Delete an agent
    Delete {
        name: String,
        /// Also delete the agent's workspace directory
        #[arg(long)]
        wipe: bool,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RestartMode {
    Restart,
    #[value(alias = "reset_session")]
    ResetSession,
    #[value(alias = "full_reset")]
    FullReset,
}

impl std::fmt::Display for RestartMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl RestartMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RestartMode::Restart => "restart",
            RestartMode::ResetSession => "reset_session",
            RestartMode::FullReset => "full_reset",
        }
    }
}

/// Subdirectory inside the data dir root that holds SQLite + per-agent/team
/// workspaces. Kept separate from `logs/` and `config.toml`.
pub const DATA_SUBDIR: &str = "data";

pub const DEFAULT_TEMPLATE_DIR: &str = "~/agency-agents";

pub fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.chorus")
}

/// Authenticated user as reported by `GET /api/whoami`. Composed from
/// the server's view of the actor for the request — that's the User the
/// credentials in `~/.chorus/credentials.toml` resolve to.
#[derive(Debug, Clone)]
pub struct AuthedUser {
    pub id: String,
    pub name: String,
}

/// Resolve the CLI's bearer token. Precedence:
///   1. `CHORUS_TOKEN` env var (used by integration tests + scripts).
///   2. `~/.chorus/credentials.toml`.
///
/// Returns `Err(CliError)` with setup guidance if neither is present.
pub fn resolve_cli_token() -> anyhow::Result<String> {
    if let Ok(env_token) = std::env::var("CHORUS_TOKEN") {
        if !env_token.trim().is_empty() {
            return Ok(env_token);
        }
    }
    let data_dir_str = default_data_dir();
    let data_dir = std::path::Path::new(&data_dir_str);
    let creds = credentials::load(data_dir)?.ok_or_else(|| {
        CliError(format!(
            "no credentials at {} (and CHORUS_TOKEN unset); run `chorus setup` or `chorus login --local`",
            credentials::path_for(data_dir).display()
        ))
    })?;
    Ok(creds.token)
}

/// Fetch the current user's `(id, name)` from a running server, sending
/// the bearer token resolved by `resolve_cli_token`. Also returns the
/// token so callers can attach `.bearer_auth(&token)` to follow-up
/// HTTP calls — the platform requires every `/internal/*` and `/api/*`
/// request to carry credentials.
///
/// Fails with setup guidance when:
///   - no token available (env or file) → tell user to run `platform setup`
///     (or `platform login --local` to mint a token against an existing
///     install)
///   - the server returns 401 → token revoked or stale; same recovery.
pub async fn fetch_authed_user_with_token(
    client: &reqwest::Client,
    server_url: &str,
) -> anyhow::Result<(AuthedUser, String)> {
    use anyhow::Context;
    let token = resolve_cli_token()?;
    let url = format!("{server_url}/api/whoami");
    let res = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(CliError(
            "authentication failed; the token may be revoked. Run `chorus login --local`".into(),
        )
        .into());
    }
    if !status.is_success() {
        return Err(anyhow::anyhow!("{status} from {url}: {body}"));
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("unexpected /api/whoami response from {server_url}"))?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| CliError("server returned empty user id".into()))?
        .to_string();
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| CliError("server returned empty user name".into()))?
        .to_string();
    Ok((AuthedUser { id, name }, token))
}

/// Back-compat shim for callers that only need the user identity. New
/// code should prefer `fetch_authed_user_with_token` so it can attach
/// the bearer to follow-up requests.
#[allow(dead_code)]
pub async fn fetch_authed_user(
    client: &reqwest::Client,
    server_url: &str,
) -> anyhow::Result<AuthedUser> {
    fetch_authed_user_with_token(client, server_url)
        .await
        .map(|(u, _)| u)
}

pub fn default_model_for_runtime(runtime: &str) -> &str {
    match AgentRuntime::parse(runtime) {
        Some(AgentRuntime::Codex) => "gpt-5.4",
        Some(AgentRuntime::Kimi) => "kimi-code/kimi-for-coding",
        Some(AgentRuntime::Gemini) => "gemini-3.1-pro-preview",
        _ => "sonnet",
    }
}

/// Resolve the effective template directory: CLI flag > config file > default.
/// (The env-var layer is folded into `cli` by clap's `env` attribute.)
pub fn resolve_template_dir(data_dir_str: &str, cli: Option<String>) -> String {
    if let Some(v) = cli {
        return v;
    }
    let data_dir = crate::agent::templates::expand_tilde(data_dir_str);
    match ChorusConfig::load(&data_dir) {
        Ok(Some(cfg)) => cfg
            .agent_template
            .dir
            .unwrap_or_else(|| DEFAULT_TEMPLATE_DIR.to_string()),
        _ => DEFAULT_TEMPLATE_DIR.to_string(),
    }
}
