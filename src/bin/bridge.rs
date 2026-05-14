//! `bridge` — Chorus bridge daemon.
//!
//! Connects a local runtime to a remote `chorus-server` over WebSocket
//! and proxies MCP tool-calls back to the platform's HTTP API. The
//! happy path is zero-arg:
//!
//!   bridge
//!
//! Reads `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml` (written
//! by the Settings → Devices one-liner on the platform), connects, and
//! hosts whatever agents the platform owns for this machine.

use clap::Parser;

use chorus::cli::bridge;
use chorus::cli::CliError;

#[derive(Parser)]
#[command(name = "bridge", about = "Chorus bridge daemon")]
struct Cli {
    /// Override the default data dir (`$XDG_DATA_HOME/chorus/bridge`).
    /// The bridge reads `bridge-credentials.toml` from here and persists
    /// `machine_id` back to it on first connect.
    #[arg(long)]
    data_dir: Option<String>,
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

    let data_dir_str = cli.data_dir.unwrap_or_else(bridge::default_bridge_data_dir);

    // Bridge logs to `<data_dir>/logs/chorus.log` + stdout. No separate
    // `--log-dir` flag — the bridge's persistence story is "one data dir
    // per machine" and logs ride along with it.
    let _log_guard = chorus::logging::init_tracing(Some(&data_dir_str));

    bridge::run(data_dir_str).await
}
