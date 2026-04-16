//! CLI entry point for the `chorus` binary.
//!
//! Declares the top-level [`Commands`] enum and [`AgentCommands`] sub-enum
//! (parsed by clap), shared helper functions (`default_data_dir`,
//! `db_path_for`, `resolve_template_dir`, …), and dispatches each subcommand
//! to its own module.

mod agent;
mod channel;
mod history;
mod send;
mod serve;
mod setup;
mod start;
mod status;

use clap::{Parser, Subcommand};

use chorus::agent::AgentRuntime;
use chorus::config::ChorusConfig;

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
    /// Start the server and open the web UI in a browser.
    /// Use `--no-open` to skip the browser (alias for the former `serve`).
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
    /// Read message history
    History {
        /// Target: #channel, dm:@name, etc.
        channel: String,
        #[arg(long, default_value = "20")]
        limit: i64,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// List channels, agents, humans
    Status {
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Create a channel
    Channel {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        data_dir: Option<String>,
    },
    /// Run as an MCP stdio bridge for a specific agent (internal use by agent manager)
    #[command(hide = true)]
    Bridge {
        #[arg(long)]
        agent_id: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Start the shared HTTP MCP bridge server (multi-agent)
    #[command(name = "bridge-serve")]
    BridgeServe {
        /// Address to listen on (e.g. 127.0.0.1:4321)
        #[arg(long, default_value = "127.0.0.1:4321")]
        listen: String,
        /// Chorus backend server URL
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Run a smoke test against a temporary bridge server.
    #[command(name = "bridge-smoke-test")]
    BridgeSmokeTest,
    /// Mint a one-time pairing token for an agent to connect to the running bridge.
    #[command(name = "bridge-pair")]
    BridgePair {
        /// Agent key to pair (matches the Chorus agent name).
        #[arg(long)]
        agent: String,
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
        /// Also start the shared MCP bridge on 127.0.0.1:<bridge_port> in the
        /// same process. Agents started while this is running will auto-connect
        /// via HTTP MCP instead of spawning per-agent stdio bridges.
        #[arg(long)]
        shared_bridge: bool,
        /// Port for the shared bridge (only used when --shared-bridge is set).
        #[arg(long, default_value = "4321")]
        bridge_port: u16,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentCommands {
    /// Create and start a new agent
    Create {
        name: String,
        #[arg(long, default_value = "claude")]
        runtime: String,
        #[arg(long, default_value = "")]
        model: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        data_dir: Option<String>,
    },
    /// Stop a running agent
    Stop {
        name: String,
        #[arg(long)]
        data_dir: Option<String>,
    },
    /// List all agents
    List {
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
}

/// Subdirectory inside the data dir root that holds SQLite + per-agent/team
/// workspaces. Kept separate from `logs/` and `config.toml`.
pub(crate) const DATA_SUBDIR: &str = "data";

pub(crate) const DEFAULT_TEMPLATE_DIR: &str = "~/agency-agents";

pub(crate) fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.chorus")
}

pub(crate) fn default_model_for_runtime(runtime: &str) -> &str {
    match AgentRuntime::parse(runtime) {
        Some(AgentRuntime::Codex) => "gpt-5.4",
        Some(AgentRuntime::Kimi) => "kimi-code/kimi-for-coding",
        _ => "sonnet",
    }
}

/// Resolve and prepare `<data_dir_root>/data/chorus.db`, creating the parent
/// directory as a side effect. Returns the path as a String for `Store::open`.
pub(crate) fn db_path_for(data_dir_root: &str) -> String {
    let dir = std::path::PathBuf::from(data_dir_root).join(DATA_SUBDIR);
    let _ = std::fs::create_dir_all(&dir);
    dir.join("chorus.db").to_string_lossy().into_owned()
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
        }) => start::run(port, data_dir, no_open, template_dir).await,

        None => {
            let data_dir_str = default_data_dir();
            let template_dir_str = resolve_template_dir(&data_dir_str, None);
            serve::run(3001, data_dir_str, template_dir_str, false, 4321).await
        }

        Some(Commands::Bridge {
            agent_id,
            server_url,
        }) => chorus::bridge::run_bridge(agent_id, server_url).await,

        Some(Commands::BridgeServe { listen, server_url }) => {
            chorus::bridge::serve::run_bridge_server(&listen, &server_url).await
        }

        Some(Commands::BridgeSmokeTest) => chorus::bridge::smoke_test::run_smoke_test().await,

        Some(Commands::BridgePair { agent }) => {
            chorus::bridge::pairing::run_bridge_pair(&agent).await
        }

        Some(Commands::Send {
            target,
            content,
            server_url,
        }) => send::run(target, content, server_url).await,

        Some(Commands::History {
            channel,
            limit,
            server_url,
        }) => history::run(channel, limit, server_url).await,

        Some(Commands::Status { server_url }) => status::run(server_url).await,

        Some(Commands::Channel {
            name,
            description,
            data_dir,
        }) => channel::run(name, description, data_dir),

        Some(Commands::Agent { cmd }) => agent::run(cmd).await,

        Some(Commands::Serve {
            port,
            data_dir,
            template_dir,
            shared_bridge,
            bridge_port,
        }) => {
            let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
            let template_dir_str = resolve_template_dir(&data_dir_str, template_dir);
            serve::run(port, data_dir_str, template_dir_str, shared_bridge, bridge_port).await
        }
    }
}
