//! `chorus` — device-side binary.
//!
//! Two roles on the same binary:
//!
//! 1. Operator CLI: `chorus setup`, `chorus agent create`, `chorus send`,
//!    etc. One-shot subcommands that hit a running `chorus-server` over
//!    HTTP. Logs to stdout.
//!
//! 2. Bridge daemon: `chorus bridge`. Long-running. Reads
//!    `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml` (written by
//!    the Settings → Devices one-liner on the platform), dials the
//!    platform's `/api/bridge/ws`, and hosts agent runtimes that the
//!    platform owns for this machine. File-logs into the bridge data
//!    dir.

use clap::{Parser, Subcommand};

use chorus::cli::{
    agent, bridge, channel, check, login, logout, send, setup, status, workspace, AgentCommands,
    CliError,
};

#[derive(Parser)]
#[command(name = "chorus", about = "Chorus device-side CLI + bridge daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    /// Run the bridge daemon — connect this machine to a remote
    /// `chorus-server` over WebSocket and host its agent runtimes
    /// locally. Zero-arg happy path: reads
    /// `bridge-credentials.toml` from `$XDG_DATA_HOME/chorus/bridge`.
    Bridge {
        /// Override the default data dir (`$XDG_DATA_HOME/chorus/bridge`).
        /// `bridge-credentials.toml` lives here; logs land in
        /// `<data_dir>/logs/`. `machine_id` is persisted to the
        /// credentials file on first connect.
        #[arg(long)]
        data_dir: Option<String>,
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

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // File-log for the long-lived `bridge` daemon and for `setup` (which
    // touches on-disk state). One-shot operator subcommands log to
    // stdout — they're HTTP clients with no persistent state.
    let log_data_dir = match &cli.command {
        Commands::Setup { data_dir, .. } => Some(
            data_dir
                .clone()
                .unwrap_or_else(chorus::cli::default_data_dir),
        ),
        Commands::Bridge { data_dir } => Some(
            data_dir
                .clone()
                .unwrap_or_else(bridge::default_bridge_data_dir),
        ),
        _ => None,
    };
    let _log_guard = chorus::logging::init_tracing(log_data_dir.as_deref());

    match cli.command {
        Commands::Setup {
            yes,
            data_dir,
            template_dir,
        } => setup::run(yes, data_dir, template_dir).await,

        Commands::Bridge { data_dir } => {
            let data_dir_str = data_dir.unwrap_or_else(bridge::default_bridge_data_dir);
            bridge::run(data_dir_str).await
        }

        Commands::Send {
            target,
            content,
            server_url,
        } => send::run(target, content, server_url).await,

        Commands::Status { server_url } => status::run(server_url).await,

        Commands::Channel { cmd, server_url } => channel::run(server_url, cmd).await,

        Commands::Workspace { server_url, cmd } => workspace::run(server_url, cmd).await,

        Commands::Agent { cmd } => agent::run(cmd).await,

        Commands::Check { data_dir } => check::run(data_dir).await,

        Commands::Login {
            local,
            data_dir,
            label,
        } => {
            if !local {
                return Err(CliError(
                    "only `chorus login --local` is supported today; cloud providers land in a later release".into(),
                )
                .into());
            }
            login::run(data_dir, label).await
        }

        Commands::Logout { data_dir } => logout::run(data_dir).await,
    }
}
