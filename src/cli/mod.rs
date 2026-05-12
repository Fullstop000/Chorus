//! CLI entry point for the `chorus` binary.
//!
//! Declares the top-level [`Commands`] enum and [`AgentCommands`] sub-enum
//! (parsed by clap), shared helper functions (`default_data_dir`,
//! `db_path_for`, `resolve_template_dir`, …), and dispatches each subcommand
//! to its own module.

mod agent;
mod bridge;
mod channel;
mod check;
pub(crate) mod credentials;
mod login;
mod logout;
mod send;
mod serve;
mod setup;
mod start;
mod status;
mod workspace;

use clap::{Parser, Subcommand, ValueEnum};

use chorus::agent::AgentRuntime;
use chorus::config::ChorusConfig;

/// An expected user error that should be printed cleanly without a backtrace.
/// The Chorus CLI enables `RUST_BACKTRACE=1` by default, so `anyhow::bail!`
/// would emit a full backtrace for mundane mistakes like forgetting `--yes`.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

#[derive(Parser)]
#[command(name = "chorus", about = "Local AI agent collaboration platform")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// First-run doctor: detect runtimes, ACP adaptors, and templates
    Setup {
        /// Non-interactive mode (skip prompts, accept defaults)
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        data_dir: Option<String>,
        /// Directory containing agent template markdown files.
        /// Precedence: CLI flag > `CHORUS_TEMPLATE_DIR` env var >
        /// `<data_dir>/config.toml` > `~/agency-agents`.
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
    },
    /// Start the server and open the web UI in a browser
    /// (use `--no-open` to skip opening a browser tab).
    Start {
        #[arg(long, default_value = "3001")]
        port: u16,
        #[arg(long)]
        data_dir: Option<String>,
        /// Do not open a browser tab
        #[arg(long)]
        no_open: bool,
        /// Directory containing agent template markdown files.
        /// Precedence: CLI flag > `CHORUS_TEMPLATE_DIR` env var >
        /// `<data_dir>/config.toml` > `~/agency-agents`.
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
        /// Port for the shared MCP bridge, started in-process.
        #[arg(long, default_value_t = chorus::bridge::DEFAULT_BRIDGE_PORT)]
        bridge_port: u16,
    },
    /// Create and manage agents
    Agent {
        #[command(subcommand)]
        cmd: AgentCommands,
    },
    /// Send a message as the human user
    Send {
        /// Target: #channel, dm:@name, etc.
        target: String,
        /// Message content
        content: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// List channels, agents, humans
    Status {
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Manage channels (create, del, join, list, history)
    Channel {
        #[command(subcommand)]
        cmd: channel::ChannelCommands,
        /// Chorus server URL (inherited by all channel subcommands)
        #[arg(long, global = true, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Manage platform workspaces
    Workspace {
        /// Chorus server URL (inherited by all workspace subcommands)
        #[arg(long, global = true, default_value = "http://localhost:3001")]
        server_url: String,
        #[command(subcommand)]
        cmd: workspace::WorkspaceCommands,
    },
    /// Start the shared HTTP MCP bridge server (multi-agent)
    #[command(name = "bridge-serve")]
    BridgeServe {
        /// Address to listen on (e.g. 127.0.0.1:4321)
        #[arg(long, default_value_t = format!("127.0.0.1:{}", chorus::bridge::DEFAULT_BRIDGE_PORT))]
        listen: String,
        /// Chorus backend server URL
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Run a remote bridge: connect to a platform via WebSocket, host
    /// local agent runtimes, and proxy MCP tool-calls back to the platform.
    Bridge {
        /// Platform WebSocket URL (e.g. ws://platform.host:3001/api/bridge/ws).
        #[arg(long)]
        platform_ws: String,
        /// Platform HTTP base URL (e.g. http://platform.host:3001) for MCP proxy.
        #[arg(long)]
        platform_http: String,
        /// Bearer token for the WS upgrade. Must match a row in the
        /// platform's `api_tokens` table with `machine_id` set to this
        /// bridge's `--machine-id`. Mint one on the platform host with
        /// `chorus tokens mint --bridge --machine-id <id>` (or use the
        /// `bridge-credentials.toml` written by `chorus setup` for the
        /// local install).
        #[arg(long, env = "CHORUS_BRIDGE_TOKEN")]
        token: Option<String>,
        /// Stable identifier for this bridge instance.
        #[arg(long)]
        machine_id: String,
        /// Local data directory for the bridge (separate from any platform data).
        #[arg(long)]
        data_dir: Option<String>,
        /// Loopback bind for the embedded MCP bridge that local agents talk to.
        #[arg(long, default_value = "127.0.0.1:0")]
        bridge_listen: String,
    },
    /// Read-only environment diagnostic
    Check {
        #[arg(long)]
        data_dir: Option<String>,
    },
    /// Mint a fresh CLI bearer token. Local mode only — talks directly to
    /// the on-disk Chorus DB.
    Login {
        /// Use the singleton local Account. Required (cloud providers
        /// will land later).
        #[arg(long)]
        local: bool,
        #[arg(long)]
        data_dir: Option<String>,
        /// Human-readable label stored on the token row for `chorus tokens
        /// list` / future audit.
        #[arg(long)]
        label: Option<String>,
    },
    /// Revoke the current CLI token and delete the local credentials file.
    Logout {
        #[arg(long)]
        data_dir: Option<String>,
    },
    /// Alias for `start --no-open` (kept for backward compatibility)
    #[command(hide = true)]
    Serve {
        #[arg(long, default_value = "3001")]
        port: u16,
        #[arg(long)]
        data_dir: Option<String>,
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
        /// Port for the shared MCP bridge, started in-process by `chorus serve`.
        #[arg(long, default_value_t = chorus::bridge::DEFAULT_BRIDGE_PORT)]
        bridge_port: u16,
        /// Deprecated: the shared bridge is now always started by
        /// `chorus serve` (there is no longer an opt-in). Accepted so existing
        /// scripts continue to work; emits a warning and is otherwise ignored.
        #[arg(long, hide = true)]
        shared_bridge: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentCommands {
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
pub(crate) enum RestartMode {
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
pub(crate) const DATA_SUBDIR: &str = "data";

pub(crate) const DEFAULT_TEMPLATE_DIR: &str = "~/agency-agents";

pub(crate) fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.chorus")
}

/// Authenticated user as reported by `GET /api/whoami`. Composed from
/// the server's view of the actor for the request — that's the User the
/// credentials in `~/.chorus/credentials.toml` resolve to.
#[derive(Debug, Clone)]
pub(crate) struct AuthedUser {
    pub id: String,
    pub name: String,
}

/// Resolve the CLI's bearer token. Precedence:
///   1. `CHORUS_TOKEN` env var (used by integration tests + scripts).
///   2. `~/.chorus/credentials.toml`.
///
/// Returns `Err(CliError)` with setup guidance if neither is present.
pub(crate) fn resolve_cli_token() -> anyhow::Result<String> {
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
///   - no token available (env or file) → tell user to run `chorus setup`
///     (or `chorus login --local` to mint a token against an existing
///     install)
///   - the server returns 401 → token revoked or stale; same recovery.
pub(crate) async fn fetch_authed_user_with_token(
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
pub(crate) async fn fetch_authed_user(
    client: &reqwest::Client,
    server_url: &str,
) -> anyhow::Result<AuthedUser> {
    fetch_authed_user_with_token(client, server_url)
        .await
        .map(|(u, _)| u)
}

pub(crate) fn default_model_for_runtime(runtime: &str) -> &str {
    match AgentRuntime::parse(runtime) {
        Some(AgentRuntime::Codex) => "gpt-5.4",
        Some(AgentRuntime::Kimi) => "kimi-code/kimi-for-coding",
        Some(AgentRuntime::Gemini) => "gemini-3.1-pro-preview",
        _ => "sonnet",
    }
}

/// Resolve the effective template directory: CLI flag > config file > default.
/// (The env-var layer is folded into `cli` by clap's `env` attribute.)
pub(crate) fn resolve_template_dir(data_dir_str: &str, cli: Option<String>) -> String {
    if let Some(v) = cli {
        return v;
    }
    let data_dir = chorus::agent::templates::expand_tilde(data_dir_str);
    match ChorusConfig::load(&data_dir) {
        Ok(Some(cfg)) => cfg
            .agent_template
            .dir
            .unwrap_or_else(|| DEFAULT_TEMPLATE_DIR.to_string()),
        _ => DEFAULT_TEMPLATE_DIR.to_string(),
    }
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Enable RUST_BACKTRACE by default so Backtrace::capture() works in error handlers.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Only serve/start and the default server case persist data → want file logging.
    // CLI-only subcommands (send, history, status, channel, agent) and bridge log to stdout only.
    let log_data_dir: Option<String> = match &cli.command {
        Some(Commands::Setup { data_dir, .. })
        | Some(Commands::Start { data_dir, .. })
        | Some(Commands::Serve { data_dir, .. }) => {
            Some(data_dir.clone().unwrap_or_else(default_data_dir))
        }
        Some(Commands::Bridge { data_dir, .. }) => Some(data_dir.clone().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.chorus-bridge")
        })),
        None => Some(default_data_dir()),
        _ => None,
    };
    let _log_guard = chorus::logging::init_tracing(log_data_dir.as_deref());

    match cli.command {
        Some(Commands::Setup {
            yes,
            data_dir,
            template_dir,
        }) => setup::run(yes, data_dir, template_dir).await,

        Some(Commands::Start {
            port,
            data_dir,
            no_open,
            template_dir,
            bridge_port,
        }) => start::run(port, data_dir, no_open, template_dir, bridge_port).await,

        None => {
            let data_dir_str = default_data_dir();
            let template_dir_str = resolve_template_dir(&data_dir_str, None);
            serve::run(3001, data_dir_str, template_dir_str, 4321).await
        }

        Some(Commands::BridgeServe { listen, server_url }) => {
            chorus::bridge::serve::run_bridge_server(&listen, &server_url).await
        }

        Some(Commands::Bridge {
            platform_ws,
            platform_http,
            token,
            machine_id,
            data_dir,
            bridge_listen,
        }) => {
            let data_dir_str = data_dir.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                format!("{home}/.chorus-bridge")
            });
            bridge::run(
                platform_ws,
                platform_http,
                token,
                machine_id,
                data_dir_str,
                bridge_listen,
            )
            .await
        }

        Some(Commands::Send {
            target,
            content,
            server_url,
        }) => send::run(target, content, server_url).await,

        Some(Commands::Status { server_url }) => status::run(server_url).await,

        Some(Commands::Channel { cmd, server_url }) => channel::run(server_url, cmd).await,

        Some(Commands::Workspace { server_url, cmd }) => workspace::run(server_url, cmd).await,

        Some(Commands::Agent { cmd }) => agent::run(cmd).await,

        Some(Commands::Check { data_dir }) => check::run(data_dir).await,

        Some(Commands::Login {
            local,
            data_dir,
            label,
        }) => {
            if !local {
                return Err(CliError(
                    "only `chorus login --local` is supported today; cloud providers land in a later release".into(),
                )
                .into());
            }
            login::run(data_dir, label).await
        }

        Some(Commands::Logout { data_dir }) => logout::run(data_dir).await,

        Some(Commands::Serve {
            port,
            data_dir,
            template_dir,
            bridge_port,
            shared_bridge,
        }) => {
            if shared_bridge {
                tracing::warn!(
                    "--shared-bridge is deprecated and has no effect. The shared MCP \
                     bridge is always started by `chorus serve`. Remove the flag from \
                     your scripts."
                );
            }
            let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
            let template_dir_str = resolve_template_dir(&data_dir_str, template_dir);
            serve::run(port, data_dir_str, template_dir_str, bridge_port).await
        }
    }
}
