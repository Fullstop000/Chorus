//! `chorus-server` — Chorus platform server daemon.
//!
//! Single-purpose daemon: HTTP API + WebSocket bridge + embedded UI in
//! one process. No subcommands. Cloud-deployable.
//!
//!   chorus-server --port 3001 --log-dir /var/log/chorus --data-dir /var/lib/chorus
//!
//! For operator actions (create agents, send messages, manage
//! workspaces, mint CLI tokens, …) use the separate `chorus` binary.
//! For the per-machine bridge daemon, use `bridge`.

use std::path::PathBuf;

use clap::Parser;

use chorus::agent::templates::expand_tilde;
use chorus::cli::{default_data_dir, resolve_template_dir, serve, CliError};

#[derive(Parser)]
#[command(name = "chorus-server", about = "Chorus platform server daemon")]
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

    let data_dir_str = cli.data_dir.unwrap_or_else(default_data_dir);
    let logs_dir: PathBuf = cli
        .log_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_tilde(&data_dir_str).join("logs"));
    let _log_guard =
        chorus::logging::init_tracing_with_logs_dir(Some(&data_dir_str), Some(&logs_dir));

    let template_dir_str = resolve_template_dir(&data_dir_str, cli.template_dir);
    serve::run(cli.port, data_dir_str, template_dir_str, cli.bridge_port).await
}
