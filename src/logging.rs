//! Tracing / logging initialization for the CLI.
//!
//! `serve` / `run` / `setup` / the default (no-subcommand) branch all want
//! file logging under `<data_dir>/logs/`. Other subcommands (bridge, send,
//! history, etc.) only need stdout. The entry point `init_tracing` handles
//! both: when a `data_dir` is given it reads `[logs]` from `config.toml`
//! and attaches a file layer; otherwise it installs only a stdout layer.
//!
//! ## Per-agent log routing
//!
//! Events emitted inside a span named `"agent"` with a `name` field get
//! routed to `<logs_dir>/agents/<name>.log` by [`AgentLogLayer`]. Events
//! outside any agent span stay in the main `chorus.log`. Agent readers
//! (see `agent::manager`) enter the span once per process and let the
//! layer handle file I/O — no explicit file handles passed through.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::{Context, Filter};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use crate::agent::templates::expand_tilde;
use crate::config::{ChorusConfig, LogsConfig};

/// Span name that `AgentLogLayer` watches for. Reader tasks enter a
/// `tracing::info_span!(AGENT_SPAN_NAME, name = %agent_name)` so every
/// event inside the scope gets routed to that agent's log file.
pub const AGENT_SPAN_NAME: &str = "agent";

/// Initialize tracing. Returns a `WorkerGuard` that must live for the
/// process lifetime so queued file writes flush cleanly on exit.
pub fn init_tracing(data_dir: Option<&str>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let logs_cfg = data_dir
        .map(expand_tilde)
        .and_then(|dir| ChorusConfig::load(&dir).ok().flatten())
        .map(|c| c.logs)
        .unwrap_or_default();

    // Per-layer filters: the stdout + main-file layers honor the user's
    // RUST_LOG / [logs].level setting, while the agent layer sees every
    // event (TRACE) regardless. That's what lets trace-level agent
    // stdout lines reach the per-agent log file without flooding the
    // console.
    let level_filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&logs_cfg.level))
    };

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_filter(level_filter());

    let Some(dir) = data_dir else {
        tracing_subscriber::registry().with(stdout_layer).init();
        return None;
    };

    let logs_dir = expand_tilde(dir).join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing_subscriber::registry().with(stdout_layer).init();
        eprintln!(
            "warning: could not create {}: {e} — file logging disabled",
            logs_dir.display()
        );
        return None;
    }

    let appender = match build_appender(&logs_dir, &logs_cfg) {
        Ok(a) => a,
        Err(e) => {
            tracing_subscriber::registry().with(stdout_layer).init();
            eprintln!("warning: file-logger build failed: {e} — file logging disabled");
            return None;
        }
    };
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    // Main chorus.log: respects RUST_LOG AND excludes events inside an
    // `agent` span (those get routed to per-agent files below).
    let main_file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_ansi(false)
        .with_filter(level_filter())
        .with_filter(ExcludeAgentSpansFilter);

    // Per-agent log: no level filter — captures the whole firehose for
    // post-hoc debugging. File routing is based on the active `agent`
    // span's `name` field.
    let agent_layer = AgentLogLayer::new(logs_dir.join("agents"))
        .with_filter(tracing_subscriber::filter::LevelFilter::TRACE);

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(main_file_layer)
        .with(agent_layer)
        .init();
    Some(guard)
}

// ---- per-agent routing ----------------------------------------------------

/// Extension stored on `agent` spans so per-event routing is O(1).
struct AgentScope {
    name: String,
}

/// Visit a span's fields looking for `name = "..."`.
#[derive(Default)]
struct AgentNameVisitor {
    name: Option<String>,
}

impl Visit for AgentNameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.name = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" && self.name.is_none() {
            // Debug-formatted strings come with surrounding quotes; strip them.
            let raw = format!("{value:?}");
            self.name = Some(raw.trim_matches('"').to_string());
        }
    }
}

/// Render an event's fields into a human-readable line. Caller prepends
/// timestamp and level.
struct EventFormatter<'a> {
    out: &'a mut String,
    wrote_message: bool,
}

impl<'a> Visit for EventFormatter<'a> {
    fn record_str(&mut self, field: &Field, value: &str) {
        use std::fmt::Write;
        if field.name() == "message" {
            self.out.push_str(value);
            self.wrote_message = true;
        } else {
            let _ = write!(self.out, " {}={}", field.name(), value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            let _ = write!(self.out, "{value:?}");
            self.wrote_message = true;
        } else {
            let _ = write!(self.out, " {}={:?}", field.name(), value);
        }
    }
}

/// Custom tracing layer: writes each event that occurs inside an `agent`
/// span to `<base>/<agent_name>.log`. Opens files on demand and keeps
/// them cached per agent name so we don't re-open on every line.
pub struct AgentLogLayer {
    base: PathBuf,
    files: Mutex<HashMap<String, Arc<Mutex<File>>>>,
}

impl AgentLogLayer {
    pub fn new(base: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&base);
        Self {
            base,
            files: Mutex::new(HashMap::new()),
        }
    }

    fn open(&self, name: &str) -> Option<Arc<Mutex<File>>> {
        // Refuse silly names that could escape the logs dir. Agent names
        // in chorus are DB-enforced `[a-z0-9_-]+`, but defense in depth.
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return None;
        }
        let mut files = match self.files.lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        if let Some(existing) = files.get(name) {
            return Some(existing.clone());
        }
        let path = self.base.join(format!("{name}.log"));
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let arc = Arc::new(Mutex::new(f));
        files.insert(name.to_string(), arc.clone());
        Some(arc)
    }
}

impl<S> Layer<S> for AgentLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != AGENT_SPAN_NAME {
            return;
        }
        let mut v = AgentNameVisitor::default();
        attrs.record(&mut v);
        let Some(name) = v.name else { return };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(AgentScope { name });
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        let agent_name = scope.from_root().find_map(|span| {
            span.extensions()
                .get::<AgentScope>()
                .map(|a| a.name.clone())
        });
        let Some(agent_name) = agent_name else {
            return;
        };
        let Some(file) = self.open(&agent_name) else {
            return;
        };

        let mut body = String::new();
        let mut fmt = EventFormatter {
            out: &mut body,
            wrote_message: false,
        };
        event.record(&mut fmt);
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let level = event.metadata().level();
        let Ok(mut guard) = file.lock() else { return };
        // Lock-held write keeps stdout/stderr lines from interleaving
        // mid-line. Errors are swallowed so a full disk can't kill the
        // reader task.
        let _ = writeln!(&mut *guard, "{ts} [{level}] {body}");
    }
}

/// Filter that excludes events occurring inside an `agent` span. Applied
/// to the main-file layer so those events land only in the per-agent log,
/// not double-logged into chorus.log.
pub struct ExcludeAgentSpansFilter;

impl<S> Filter<S> for ExcludeAgentSpansFilter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn enabled(&self, _meta: &tracing::Metadata<'_>, ctx: &Context<'_, S>) -> bool {
        !in_agent_scope(ctx)
    }
}

fn in_agent_scope<S>(ctx: &Context<'_, S>) -> bool
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(scope) = ctx.lookup_current() else {
        return false;
    };
    scope
        .scope()
        .any(|span| span.extensions().get::<AgentScope>().is_some())
}

// ---------------------------------------------------------------------------

fn build_appender(
    logs_dir: &Path,
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
