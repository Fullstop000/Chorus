//! CLI entry point for the `chorus` binary.
//!
//! Declares the top-level [`Commands`] enum and [`AgentCommands`] sub-enum
//! (parsed by clap), shared helper functions (`default_data_dir`,
//! `db_path_for`, `resolve_template_dir`, …), and dispatches each subcommand
//! to its own module.

mod agent;
mod channel;
mod check;
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
pub struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UserError {}

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
    /// Read-only environment diagnostic
    Check {
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

/// Local human identity as reported by `GET /api/whoami`. The CLI used to use
/// `whoami::username()` as identity, which conflated the OS user running the
/// CLI process with the Chorus human row. The server is now the source of
/// truth: it resolves identity from `ChorusConfig::local_human` (or seeds a
/// fresh `humans.id` on first run), and the CLI reads it back over HTTP.
#[derive(Debug, Clone)]
pub(crate) struct LocalHumanIdentity {
    pub id: String,
    pub name: String,
}

/// Fetch the local human's `(id, name)` from a running server.
///
/// Fails with setup guidance when the server returns a malformed or empty
/// response — the plan requires CLI human actions to refuse to fall back to
/// the OS username, so we surface a clear error instead of silently using a
/// wrong identity.
pub(crate) async fn fetch_local_human_identity(
    client: &reqwest::Client,
    server_url: &str,
) -> anyhow::Result<LocalHumanIdentity> {
    use anyhow::Context;
    let url = format!("{server_url}/api/whoami");
    let res = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "{status} from {url}: {body}; run `chorus setup` to initialize local identity"
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("unexpected /api/whoami response from {server_url}"))?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            UserError(
                "server has no local human id; run `chorus setup` to initialize local identity"
                    .into(),
            )
        })?
        .to_string();
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            UserError(
                "server has no local human name; run `chorus setup` to initialize local identity"
                    .into(),
            )
        })?
        .to_string();
    Ok(LocalHumanIdentity { id, name })
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
