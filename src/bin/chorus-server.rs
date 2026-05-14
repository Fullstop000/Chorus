//! `chorus-server` — Chorus platform binary.
//!
//! Runs the HTTP API + WebSocket bridge + embedded UI in a single
//! process. Top-level invocation runs the server; admin subcommands
//! mirror the pre-split CLI (`setup`, `agent`, `send`, …).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use chorus::agent::templates::expand_tilde;
use chorus::cli::{
    agent, channel, check, default_data_dir, login, logout, resolve_template_dir, send, serve,
    setup, start, status, workspace, AgentCommands, CliError,
};

#[derive(Parser)]
#[command(
    name = "chorus-server",
    about = "Chorus platform: HTTP API + embedded UI"
)]
struct Cli {
    /// HTTP listen port for the platform API + web UI.
    #[arg(long, default_value = "3001")]
    port: u16,
    /// Directory for tracing log files. Defaults to `<data_dir>/logs`.
    #[arg(long)]
    log_dir: Option<String>,
    /// Data directory (SQLite + agent workspaces). Defaults to `~/.chorus`.
    #[arg(long)]
    data_dir: Option<String>,
    /// Directory containing agent template markdown files.
    /// Precedence: CLI flag > `CHORUS_TEMPLATE_DIR` env var >
    /// `<data_dir>/config.toml` > `~/agency-agents`.
    #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
    template_dir: Option<String>,
    /// Port for the in-process shared MCP bridge.
    #[arg(long, default_value_t = chorus::bridge::DEFAULT_BRIDGE_PORT)]
    bridge_port: u16,
    /// After the server is reachable, open the web UI in the default browser.
    /// Off by default — cloud deploys don't want a browser; pass for local dev.
    #[arg(long)]
    open: bool,
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
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        if let Some(user_err) = e.downcast_ref::<CliError>() {
            eprintln!("Error: {user_err}");
        } else {
            eprintln!("{:?}", e);
        }
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Enable RUST_BACKTRACE so Backtrace::capture() works in error handlers.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // File logging is enabled for the run-server branch and `setup`.
    // Admin subcommands log to stdout only.
    let (log_data_dir, logs_dir): (Option<String>, Option<PathBuf>) = match &cli.command {
        Some(Commands::Setup { data_dir, .. }) => {
            let dd = data_dir.clone().unwrap_or_else(default_data_dir);
            let logs = expand_tilde(&dd).join("logs");
            (Some(dd), Some(logs))
        }
        None => {
            let dd = cli.data_dir.clone().unwrap_or_else(default_data_dir);
            let logs = cli
                .log_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| expand_tilde(&dd).join("logs"));
            (Some(dd), Some(logs))
        }
        _ => (None, None),
    };
    let _log_guard = chorus::logging::init_tracing_with_logs_dir(
        log_data_dir.as_deref(),
        logs_dir.as_deref(),
    );

    match cli.command {
        Some(Commands::Setup {
            yes,
            data_dir,
            template_dir,
        }) => setup::run(yes, data_dir, template_dir).await,

        None => {
            let data_dir_str = cli.data_dir.unwrap_or_else(default_data_dir);
            let template_dir_str = resolve_template_dir(&data_dir_str, cli.template_dir);
            if cli.open {
                start::run(
                    cli.port,
                    Some(data_dir_str),
                    false,
                    Some(template_dir_str),
                    cli.bridge_port,
                )
                .await
            } else {
                serve::run(cli.port, data_dir_str, template_dir_str, cli.bridge_port).await
            }
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

        Some(Commands::Login {
            local,
            data_dir,
            label,
        }) => {
            if !local {
                return Err(CliError(
                    "only `chorus-server login --local` is supported today; cloud providers land in a later release".into(),
                )
                .into());
            }
            login::run(data_dir, label).await
        }

        Some(Commands::Logout { data_dir }) => logout::run(data_dir).await,
    }
}
