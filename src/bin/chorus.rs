//! `chorus` — local operator CLI.
//!
//! Talks to a running `chorus-server` over HTTP. Subcommands cover
//! first-run setup, agent management, channel/workspace admin, sending
//! messages, and local credential management. Logs to stdout only —
//! file logging belongs to the server daemon, not to a one-shot CLI.

use clap::{Parser, Subcommand};

use chorus::cli::{
    agent, channel, check, login, logout, send, setup, status, workspace, AgentCommands, CliError,
};

#[derive(Parser)]
#[command(name = "chorus", about = "Chorus operator CLI")]
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

    // `setup` writes config and creates directories under the data dir;
    // route its logging into `<data_dir>/logs/` so first-run problems are
    // diagnosable. All other subcommands log to stdout — they're
    // one-shot HTTP clients with no persistent state to file-log.
    let log_data_dir = match &cli.command {
        Commands::Setup { data_dir, .. } => Some(
            data_dir
                .clone()
                .unwrap_or_else(chorus::cli::default_data_dir),
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
