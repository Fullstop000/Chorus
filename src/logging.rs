//! Tracing / logging initialization for the CLI.
//!
//! `serve` / `run` / `setup` / the default (no-subcommand) branch all want
//! file logging under `<data_dir>/logs/`. Other subcommands (bridge, send,
//! history, etc.) only need stdout. The entry point `init_tracing` handles
//! both: when a `data_dir` is given it reads `[logs]` from `config.toml`
//! and attaches a file layer; otherwise it installs only a stdout layer.

use tracing_subscriber::prelude::*;

use crate::agent::templates::expand_tilde;
use crate::config::{ChorusConfig, LogsConfig};

/// Initialize tracing. Returns a `WorkerGuard` that must live for the
/// process lifetime so queued file writes flush cleanly on exit.
pub fn init_tracing(data_dir: Option<&str>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let logs_cfg = data_dir
        .map(expand_tilde)
        .and_then(|dir| ChorusConfig::load(&dir).ok().flatten())
        .map(|c| c.logs)
        .unwrap_or_default();

    // RUST_LOG env wins over config so power-users can still dial verbosity
    // ad-hoc without editing config.toml.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&logs_cfg.level));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_file(true)
        .with_line_number(true);

    let Some(dir) = data_dir else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .init();
        return None;
    };

    let logs_dir = expand_tilde(dir).join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .init();
        eprintln!(
            "warning: could not create {}: {e} — file logging disabled",
            logs_dir.display()
        );
        return None;
    }

    let appender = match build_appender(&logs_dir, &logs_cfg) {
        Ok(a) => a,
        Err(e) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .init();
            eprintln!("warning: file-logger build failed: {e} — file logging disabled");
            return None;
        }
    };
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_ansi(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
    Some(guard)
}

fn build_appender(
    logs_dir: &std::path::Path,
    cfg: &LogsConfig,
) -> anyhow::Result<tracing_appender::rolling::RollingFileAppender> {
    let rotation = match cfg.rotation.as_str() {
        "hourly" => tracing_appender::rolling::Rotation::HOURLY,
        "daily" => tracing_appender::rolling::Rotation::DAILY,
        _ => tracing_appender::rolling::Rotation::NEVER,
    };
    let mut builder = tracing_appender::rolling::Builder::new()
        .rotation(rotation)
        .filename_prefix("chorus")
        .filename_suffix("log");
    if cfg.retention > 0 && cfg.rotation != "never" {
        builder = builder.max_log_files(cfg.retention as usize);
    }
    Ok(builder.build(logs_dir)?)
}
