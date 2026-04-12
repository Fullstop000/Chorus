use clap::{Parser, Subcommand};
use std::sync::Arc;

use chorus::agent::manager::AgentManager;
use chorus::agent::AgentRuntime;
use chorus::bridge;
use chorus::store::agents::AgentStatus;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::Store;

#[derive(Parser)]
#[command(name = "chorus", about = "Local AI agent collaboration platform")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server + agent manager (default if no subcommand)
    Serve {
        #[arg(long, default_value = "3001")]
        port: u16,
        #[arg(long)]
        data_dir: Option<String>,
        /// Directory containing agent template markdown files.
        /// Falls back to `<data_dir>/config.toml` then CHORUS_TEMPLATE_DIR
        /// then `~/agency-agents`.
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
    },
    /// First-run doctor: detect runtimes, ACP adaptors, and templates
    Setup {
        /// Non-interactive mode (skip prompts, accept defaults)
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        data_dir: Option<String>,
        /// Directory containing agent template markdown files.
        /// Falls back to `<data_dir>/config.toml` then CHORUS_TEMPLATE_DIR
        /// then `~/agency-agents`.
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
    },
    /// Start the server and open the web UI in a browser
    Run {
        #[arg(long, default_value = "3001")]
        port: u16,
        #[arg(long)]
        data_dir: Option<String>,
        /// Do not open a browser tab
        #[arg(long)]
        no_open: bool,
        /// Directory containing agent template markdown files.
        /// Falls back to `<data_dir>/config.toml` then CHORUS_TEMPLATE_DIR
        /// then `~/agency-agents`.
        #[arg(long, env = "CHORUS_TEMPLATE_DIR")]
        template_dir: Option<String>,
    },
    /// Run as MCP chat bridge (spawned by agent processes)
    Bridge {
        #[arg(long)]
        agent_id: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
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
}

#[derive(Subcommand)]
enum AgentCommands {
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

fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.chorus")
}

fn default_model_for_runtime(runtime: &str) -> &str {
    match AgentRuntime::parse(runtime) {
        Some(AgentRuntime::Codex) => "gpt-5.4",
        Some(AgentRuntime::Kimi) => "kimi-code/kimi-for-coding",
        _ => "sonnet",
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Enable RUST_BACKTRACE by default so Backtrace::capture() works in error handlers.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Only serve/run/setup/default (no subcommand) persist data and therefore
    // want file logging. CLI-only subcommands (bridge, send, history, status,
    // channel, agent) log to stdout only.
    let log_data_dir: Option<String> = match &cli.command {
        Some(Commands::Serve { data_dir, .. })
        | Some(Commands::Setup { data_dir, .. })
        | Some(Commands::Run { data_dir, .. }) => {
            Some(data_dir.clone().unwrap_or_else(default_data_dir))
        }
        None => Some(default_data_dir()),
        _ => None,
    };
    let _log_guard = chorus::logging::init_tracing(log_data_dir.as_deref());

    match cli.command {
        Some(Commands::Bridge {
            agent_id,
            server_url,
        }) => bridge::run_bridge(agent_id, server_url).await,

        Some(Commands::Serve {
            port,
            data_dir,
            template_dir,
        }) => {
            let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
            let template_dir_str = resolve_template_dir(&data_dir_str, template_dir);
            serve(port, data_dir_str, template_dir_str).await
        }

        Some(Commands::Setup {
            yes,
            data_dir,
            template_dir,
        }) => cmd_setup(yes, data_dir, template_dir).await,

        Some(Commands::Run {
            port,
            data_dir,
            no_open,
            template_dir,
        }) => cmd_run(port, data_dir, no_open, template_dir).await,

        None => {
            let data_dir_str = default_data_dir();
            let template_dir_str = resolve_template_dir(&data_dir_str, None);
            serve(3001, data_dir_str, template_dir_str).await
        }

        Some(Commands::Send {
            target,
            content,
            server_url,
        }) => {
            let username = whoami::username();
            let client = reqwest::Client::new();
            let res = client
                .post(format!("{server_url}/internal/agent/{username}/send"))
                .json(&serde_json::json!({ "target": target, "content": content }))
                .send()
                .await?;
            let data: serde_json::Value = res.json().await?;
            if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
                eprintln!("Error: {err}");
            } else {
                let msg_id = data
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                println!("Message sent to {target}. ID: {msg_id}");
            }
            Ok(())
        }

        Some(Commands::History {
            channel,
            limit,
            server_url,
        }) => {
            let username = whoami::username();
            let client = reqwest::Client::new();
            let res = client
                .get(format!(
                    "{server_url}/internal/agent/{username}/history?channel={}&limit={limit}",
                    urlencoding::encode(&channel)
                ))
                .send()
                .await?;
            let data: serde_json::Value = res.json().await?;
            if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
                eprintln!("Error: {err}");
            } else if let Some(messages) = data.get("messages").and_then(|v| v.as_array()) {
                if messages.is_empty() {
                    println!("No messages.");
                } else {
                    for m in messages {
                        let sender = m.get("senderName").and_then(|v| v.as_str()).unwrap_or("?");
                        let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let time = m.get("createdAt").and_then(|v| v.as_str()).unwrap_or("");
                        println!("[{time}] @{sender}: {content}");
                    }
                }
            }
            Ok(())
        }

        Some(Commands::Status { server_url }) => {
            let username = whoami::username();
            let client = reqwest::Client::new();
            let res = client
                .get(format!("{server_url}/internal/agent/{username}/server"))
                .send()
                .await?;
            let data: serde_json::Value = res.json().await?;
            println!("== Channels ==");
            if let Some(channels) = data.get("channels").and_then(|v| v.as_array()) {
                for ch in channels {
                    let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let joined = ch.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
                    let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let status = if joined { "joined" } else { "not joined" };
                    if desc.is_empty() {
                        println!("  #{name} [{status}]");
                    } else {
                        println!("  #{name} [{status}] — {desc}");
                    }
                }
            }
            println!("\n== Agents ==");
            if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
                for a in agents {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("  @{name} ({status})");
                }
            }
            println!("\n== Humans ==");
            if let Some(humans) = data.get("humans").and_then(|v| v.as_array()) {
                for h in humans {
                    let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("  @{name}");
                }
            }
            Ok(())
        }

        Some(Commands::Channel {
            name,
            description,
            data_dir,
        }) => {
            let username = whoami::username();
            let data_dir = data_dir.unwrap_or_else(default_data_dir);
            let db_path = db_path_for(&data_dir);
            let store = Store::open(&db_path)?;
            store.create_channel(&name, description.as_deref(), ChannelType::Channel)?;
            store.join_channel(&name, &username, SenderType::Human)?;
            println!("Channel #{name} created.");
            Ok(())
        }

        Some(Commands::Agent { cmd }) => {
            match cmd {
                AgentCommands::Create {
                    name,
                    runtime,
                    model,
                    description,
                    data_dir,
                } => {
                    let model = if model.is_empty() {
                        default_model_for_runtime(&runtime).to_string()
                    } else {
                        model
                    };
                    let data_dir = data_dir.unwrap_or_else(default_data_dir);
                    let db_path = db_path_for(&data_dir);
                    let store = Store::open(&db_path)?;
                    store.create_agent_record(
                        &name,
                        &name,
                        description.as_deref(),
                        &runtime,
                        &model,
                        &[],
                    )?;
                    // Join default channels
                    for ch in store.get_channels()? {
                        let _ = store.join_channel(&ch.name, &name, SenderType::Agent);
                    }
                    println!("Agent @{name} created (runtime: {runtime}, model: {model}).");
                    println!("Start it by running the server: `chorus serve`");
                    Ok(())
                }
                AgentCommands::Stop { name, data_dir } => {
                    println!("Stopping agent @{name}...");
                    let data_dir = data_dir.unwrap_or_else(default_data_dir);
                    let db_path = db_path_for(&data_dir);
                    let store = Store::open(&db_path)?;
                    store.update_agent_status(&name, AgentStatus::Inactive)?;
                    println!("Agent @{name} marked as inactive.");
                    Ok(())
                }
                AgentCommands::List { server_url } => {
                    let client = reqwest::Client::new();
                    let username = whoami::username();
                    let res = client
                        .get(format!("{server_url}/internal/agent/{username}/server"))
                        .send()
                        .await?;
                    let data: serde_json::Value = res.json().await?;
                    if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
                        if agents.is_empty() {
                            println!("No agents.");
                        } else {
                            for a in agents {
                                let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let status =
                                    a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                                let runtime =
                                    a.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
                                println!("  @{name} [{status}] (runtime: {runtime})");
                            }
                        }
                    }
                    Ok(())
                }
            }
        }
    }
}

async fn serve(port: u16, data_dir_str: String, template_dir_raw: String) -> anyhow::Result<()> {
    let data_dir = std::path::PathBuf::from(&data_dir_str);
    let data_subdir = data_dir.join(DATA_SUBDIR);
    let logs_dir = data_dir.join("logs");
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&logs_dir)?;
    std::fs::create_dir_all(&agents_dir)?;
    let db_path = data_subdir.join("chorus.db");
    let store =
        Arc::new(Store::open(db_path.to_str().unwrap())?.with_agents_dir(agents_dir.clone()));

    // Default human = OS username
    let username = whoami::username();
    let _ = store.create_human(&username);

    // Ensure built-in system channels exist and upgrade legacy installs to #all.
    store.ensure_builtin_channels(&username)?;

    let server_url = format!("http://localhost:{port}");
    let bridge_binary = std::env::current_exe()?.to_string_lossy().into_owned();
    let manager = Arc::new(AgentManager::new(
        store.clone(),
        agents_dir,
        bridge_binary,
        server_url.clone(),
    ));

    // Auto-restart agents that were active before server restart.
    // Track failures per agent so repeated failures can be surfaced.
    {
        let active_agents: Vec<String> = store
            .get_agents()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.status == AgentStatus::Active)
            .map(|a| a.name)
            .collect();
        let mut failed_agents = Vec::new();
        for agent_name in active_agents {
            tracing::info!(agent = %agent_name, "auto-restarting active agent");
            if let Err(e) = manager.start_agent(&agent_name, None).await {
                let error_detail = format!("{e:#}");
                tracing::error!(agent = %agent_name, err = %error_detail, "failed to restart agent — marking inactive so subsequent delivery can retry");
                // Mark inactive so next message delivery can attempt a fresh start
                if let Err(e) = store.update_agent_status(&agent_name, AgentStatus::Inactive) {
                    tracing::error!(agent = %agent_name, err = %e, "also failed to mark agent inactive — manual intervention required");
                }
                failed_agents.push((agent_name, error_detail));
            }
        }
        if !failed_agents.is_empty() {
            eprintln!(
                "Warning: {} agent(s) failed to auto-restart and were marked inactive: {}",
                failed_agents.len(),
                failed_agents
                    .iter()
                    .map(|(agent_name, _)| agent_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for (agent_name, error_detail) in &failed_agents {
                eprintln!("  - {agent_name}: {error_detail}");
            }
            eprintln!("They will be retried on next message delivery. To restart immediately: `chorus agent start <name>`");
        }
    }

    // Load agent templates from the configured directory.
    let template_path = chorus::agent::templates::expand_tilde(&template_dir_raw);
    let templates = chorus::agent::templates::load_templates(&template_path);

    let router = chorus::server::build_router_with_services(
        store.clone(),
        manager.clone(),
        Arc::new(chorus::agent::runtime_status::SystemRuntimeStatusProvider)
            as chorus::agent::runtime_status::SharedRuntimeStatusProvider,
        templates,
    );

    // Spawn background trace writer for Telescope persistence.
    chorus::store::trace_writer::spawn_trace_writer(
        db_path.to_str().unwrap().to_string(),
        store.subscribe_traces(),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("Chorus running at {server_url}");
    println!("Human user: @{username}");
    println!("Use `chorus send '#all' 'hello'` to send messages");
    println!("Use `chorus agent create <name>` to create an agent");

    // Graceful shutdown on Ctrl+C
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        println!("\nShutting down...");
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

// ---- chorus setup / chorus run -------------------------------------------

use chorus::config::ChorusConfig;
use console::{style, Emoji};
use std::io::IsTerminal;
use std::process::Command;
use std::time::{Duration, Instant};

const DEFAULT_TEMPLATE_DIR: &str = "~/agency-agents";

/// Subdirectory inside the data dir root that holds SQLite + per-agent/team
/// workspaces. Kept separate from `logs/` and `config.toml`.
const DATA_SUBDIR: &str = "data";

/// Resolve and prepare `<data_dir_root>/data/chorus.db`, creating the parent
/// directory as a side effect. Returns the path as a String for `Store::open`.
fn db_path_for(data_dir_root: &str) -> String {
    let dir = std::path::PathBuf::from(data_dir_root).join(DATA_SUBDIR);
    let _ = std::fs::create_dir_all(&dir);
    dir.join("chorus.db").to_string_lossy().into_owned()
}

/// Reconcile older installs with the current layout:
///   root → <root>/data/   : chorus.db*, attachments/, teams/
///   <root>/data/ → root   : agents/  (an earlier commit mistakenly moved it)
/// Only moves when the source exists and the target doesn't — idempotent,
/// refuses to clobber.
fn migrate_legacy_layout(
    root: &std::path::Path,
    data_subdir: &std::path::Path,
) -> anyhow::Result<()> {
    // Things that live under <root>/data/ going forward.
    let into_data = [
        "chorus.db",
        "chorus.db-wal",
        "chorus.db-shm",
        "attachments",
        "teams",
    ];
    for name in into_data {
        let src = root.join(name);
        let dst = data_subdir.join(name);
        if src.exists() && !dst.exists() {
            tracing::info!(from = %src.display(), to = %dst.display(), "migrating legacy layout");
            std::fs::rename(&src, &dst)?;
        }
    }
    // Undo the misplacement of agents/ by pulling it back to the root.
    let stray = data_subdir.join("agents");
    let home = root.join("agents");
    if stray.exists() && !home.exists() {
        tracing::info!(from = %stray.display(), to = %home.display(), "restoring agents/ to root");
        std::fs::rename(&stray, &home)?;
    }
    Ok(())
}

/// Resolve the effective template directory: CLI flag > config file > default.
/// (The env-var layer is folded into `cli` by clap's `env` attribute.)
fn resolve_template_dir(data_dir_str: &str, cli: Option<String>) -> String {
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

// Glyphs. `console::Emoji` falls back to ASCII on dumb terminals.
static OK: Emoji<'_, '_> = Emoji("✓ ", "ok ");
static BAD: Emoji<'_, '_> = Emoji("✗ ", "x  ");
static WARN: Emoji<'_, '_> = Emoji("⚠ ", "!  ");

fn banner() {
    // Render visible content for each inner row at a fixed width, then apply
    // ANSI styling on top (styling adds bytes but no visible columns).
    const INNER: usize = 41;
    let dashes = "─".repeat(INNER);
    let row1_plain = format!(
        "{:<width$}",
        " Chorus · local AI agent platform",
        width = INNER
    );
    let row1_styled = row1_plain
        .replacen("Chorus", &style("Chorus").bold().cyan().to_string(), 1)
        .replacen(
            "· local AI agent platform",
            &style("· local AI agent platform").dim().to_string(),
            1,
        );
    let row2_styled = style(format!("{:<width$}", " first-run setup", width = INNER))
        .dim()
        .to_string();
    let bar = style("│").dim();
    println!();
    println!("  {}", style(format!("┌{}┐", dashes)).dim());
    println!("  {}{}{}", bar, row1_styled, bar);
    println!("  {}{}{}", bar, row2_styled, bar);
    println!("  {}", style(format!("└{}┘", dashes)).dim());
    println!();
}

fn section(title: &str) {
    println!();
    println!("  {}", style(title).bold());
}

fn row_ok(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(OK).green(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_warn(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(WARN).yellow(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_info(label: &str, value: &str) {
    println!("  {:<12} {}", style(label).dim(), value);
}

fn footer(elapsed: Duration, next: &str) {
    println!();
    println!("  {}", style("─".repeat(41)).dim());
    println!(
        "  All set in {}. Next:",
        style(format!("{:.1}s", elapsed.as_secs_f64())).bold()
    );
    println!("    {} {}", style("$").dim(), style(next).cyan().bold());
    println!();
}

/// Extract the first dotted version number from a tool's `--version` output,
/// so we show "1.3.12" instead of "kimi, version 1.31.0".
fn extract_version(s: &str) -> Option<String> {
    static VERSION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = VERSION_RE
        .get_or_init(|| regex::Regex::new(r"\b\d+\.\d+(?:\.\d+)?(?:[-+][\w.]+)?\b").unwrap());
    re.find(s).map(|m| m.as_str().to_string())
}

/// Resolve an executable's absolute path by walking `$PATH`, the same way
/// `which <name>` does. Returns `None` if the binary isn't found.
fn which_tool(name: &str) -> Option<std::path::PathBuf> {
    which_all(name).into_iter().next()
}

/// Return every absolute path where `name` is found on `$PATH`, deduped,
/// in discovery order. Empty vec if nothing found.
fn which_all(name: &str) -> Vec<std::path::PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .filter(|p| p.is_file())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Fill `target` with the resolved absolute path for `name` iff `target`
/// is currently empty. Preserves any user-pinned value across re-runs.
/// Always uses the first match; intended for ACP adapters where a silent
/// choice is fine.
fn fill_resolved_path(target: &mut String, name: &str) {
    if target.is_empty() {
        if let Some(p) = which_tool(name) {
            *target = p.to_string_lossy().into_owned();
        }
    }
}

/// Like `fill_resolved_path`, but when multiple matches exist and we're in
/// interactive mode, ask the user to pick one. Non-interactive mode falls
/// back to the first match (current behavior).
fn fill_binary_path(target: &mut String, name: &str, interactive: bool) {
    if !target.is_empty() {
        return; // user-pinned, preserve
    }
    let candidates = which_all(name);
    let chosen = match candidates.len() {
        0 => None,
        1 => candidates.into_iter().next(),
        _ if !interactive => candidates.into_iter().next(),
        _ => {
            use dialoguer::theme::ColorfulTheme;
            use dialoguer::Select;
            let labels: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
            let idx = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Multiple `{name}` binaries on PATH; pick one"))
                .items(&labels)
                .default(0)
                .interact()
                .unwrap_or(0);
            candidates.into_iter().nth(idx)
        }
    };
    if let Some(p) = chosen {
        *target = p.to_string_lossy().into_owned();
    }
}

/// Run `<name> --version` and return the extracted dotted version, or `None`
/// if the binary is missing or the command fails. Some tools print their
/// version to stderr (historically `python --version` did), so we fall
/// back to stderr if stdout is empty.
fn check_tool(name: &str) -> Option<String> {
    let output = Command::new(name)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let source = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    extract_version(&source).or_else(|| {
        source
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// What kind of ACP support a runtime has.
enum AcpStatus {
    /// External adaptor binary is on PATH.
    AdapterFound(&'static str),
    /// External adaptor binary is missing; chorus will fall back to raw mode.
    AdapterMissing(&'static str),
    /// Runtime provides its own `acp` subcommand; nothing to install.
    Native,
}

struct RuntimeReport {
    name: &'static str,
    hint: &'static str,
    version: Option<String>,
    acp: AcpStatus,
}

fn check_runtime(name: &'static str, hint: &'static str, acp: AcpStatus) -> RuntimeReport {
    let version = check_tool(name);
    // If an external adaptor is expected, re-resolve at check time so PATH
    // changes between test runs are reflected.
    let acp = match acp {
        AcpStatus::AdapterFound(bin) | AcpStatus::AdapterMissing(bin) => {
            if Command::new(bin)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                AcpStatus::AdapterFound(bin)
            } else {
                AcpStatus::AdapterMissing(bin)
            }
        }
        AcpStatus::Native => AcpStatus::Native,
    };
    RuntimeReport {
        name,
        hint,
        version,
        acp,
    }
}

fn render_runtime(r: &RuntimeReport) {
    let (glyph, glyph_style): (Emoji<'_, '_>, _) = match &r.version {
        Some(_) => (OK, "green"),
        None => (BAD, "red"),
    };
    let glyph_styled = match glyph_style {
        "green" => style(glyph).green(),
        _ => style(glyph).red(),
    };
    let name = style(format!("{:<12}", r.name)).bold();
    let version = match &r.version {
        Some(v) => style(format!("{:<10}", v)).dim().to_string(),
        None => style(format!("{:<10}", "not found")).dim().to_string(),
    };
    let acp_detail = match (&r.version, &r.acp) {
        (None, _) => style(format!("install: {}", r.hint))
            .dim()
            .italic()
            .to_string(),
        (Some(_), AcpStatus::AdapterFound(bin)) => {
            format!(
                "{} {} {}",
                style("·").dim(),
                style(bin).cyan(),
                style("found").dim()
            )
        }
        (Some(_), AcpStatus::AdapterMissing(bin)) => {
            format!(
                "{} {} {} {}",
                style("·").dim(),
                style(bin).yellow(),
                style("missing").yellow(),
                style("→ raw mode").dim()
            )
        }
        (Some(_), AcpStatus::Native) => {
            format!("{} {}", style("·").dim(), style("native ACP").dim())
        }
    };
    println!("  {}{} {} {}", glyph_styled, name, version, acp_detail);
}

fn check_template_dir(dir: &std::path::Path) -> (usize, usize) {
    if !dir.is_dir() {
        return (0, 0);
    }
    let mut templates = 0usize;
    let mut categories = 0usize;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let mut has_md = false;
            if let Ok(sub) = std::fs::read_dir(&path) {
                for s in sub.flatten() {
                    if s.path().extension().and_then(|e| e.to_str()) == Some("md") {
                        templates += 1;
                        has_md = true;
                    }
                }
            }
            if has_md {
                categories += 1;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            templates += 1;
        }
    }
    (templates, categories)
}

async fn cmd_setup(
    yes: bool,
    data_dir: Option<String>,
    template_dir_cli: Option<String>,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let interactive = !yes && std::io::stdin().is_terminal();
    if !yes && !interactive {
        println!(
            "  {} stdin is not a terminal; running in non-interactive mode.",
            style(WARN).yellow()
        );
    }

    banner();

    // Data dir: respect --data-dir if given, otherwise prompt (with default)
    // when interactive, or silently use the default when not.
    let data_dir_str = match data_dir {
        Some(s) => s,
        None if interactive => {
            use dialoguer::theme::ColorfulTheme;
            use dialoguer::Input;
            Input::<String>::with_theme(&ColorfulTheme::default())
                .with_prompt("Data directory")
                .default(default_data_dir())
                .interact_text()
                .unwrap_or_else(|_| default_data_dir())
        }
        None => default_data_dir(),
    };
    let data_dir = chorus::agent::templates::expand_tilde(&data_dir_str);

    // Template dir precedence: CLI flag > existing config > default.
    // Setup always writes the result back so `chorus run` picks it up later.
    let template_dir_raw = template_dir_cli
        .or_else(|| {
            ChorusConfig::load(&data_dir)
                .ok()
                .flatten()
                .and_then(|c| c.agent_template.dir)
        })
        .unwrap_or_else(|| DEFAULT_TEMPLATE_DIR.to_string());
    let template_dir = chorus::agent::templates::expand_tilde(&template_dir_raw);

    row_info("Data dir", &style(data_dir.display()).cyan().to_string());
    row_info(
        "Templates",
        &style(template_dir.display()).cyan().to_string(),
    );

    // Runtimes + their ACP adaptor status
    section("Runtimes");
    let runtimes = [
        check_runtime(
            "claude",
            "https://docs.claude.com/en/docs/claude-code",
            AcpStatus::AdapterMissing("claude-agent-acp"),
        ),
        check_runtime(
            "codex",
            "https://github.com/openai/codex",
            AcpStatus::AdapterMissing("codex-acp"),
        ),
        check_runtime(
            "kimi",
            "https://github.com/MoonshotAI/kimi-cli",
            AcpStatus::Native,
        ),
        check_runtime("opencode", "https://opencode.ai", AcpStatus::Native),
    ];
    for r in &runtimes {
        render_runtime(r);
    }
    let any_adapter_missing = runtimes
        .iter()
        .any(|r| r.version.is_some() && matches!(r.acp, AcpStatus::AdapterMissing(_)));
    if any_adapter_missing {
        println!(
            "  {} {} {}",
            style(" ").dim(),
            style("ACP adaptors:").dim(),
            style("https://github.com/openclaw/acpx").dim().italic()
        );
    }
    let detected_runtimes: Vec<&str> = runtimes
        .iter()
        .filter(|r| r.version.is_some())
        .map(|r| r.name)
        .collect();

    // 3. Templates
    section("Templates");
    let (tmpl_count, tmpl_cats) = check_template_dir(&template_dir);
    if !template_dir.exists() {
        row_warn(
            "templates",
            &format!(
                "{} not found · starter gallery will be empty",
                template_dir.display()
            ),
        );
    } else if tmpl_count == 0 {
        row_warn(
            "templates",
            &format!(
                "{} exists but contains no .md files",
                template_dir.display()
            ),
        );
    } else {
        row_ok(
            "templates",
            &format!(
                "{} templates across {} categor{}",
                tmpl_count,
                tmpl_cats,
                if tmpl_cats == 1 { "y" } else { "ies" }
            ),
        );
    }

    // Ensure the directory layout exists and migrate any old-layout files
    // that were created before `data/` and `logs/` became first-class.
    //   <root>/config.toml           (config — stays at root)
    //   <root>/logs/                 (new: log files)
    //   <root>/data/                 (new: all data)
    //       chorus.db*, agents/, attachments/, teams/
    let data_subdir = data_dir.join(DATA_SUBDIR);
    let logs_dir = data_dir.join("logs");
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&logs_dir)?;
    // Migrate BEFORE creating an empty root `agents/`, otherwise the
    // reverse move would see the target already exists and skip.
    migrate_legacy_layout(&data_dir, &data_subdir)?;
    std::fs::create_dir_all(&agents_dir)?;

    // Always call Store::open: it runs migrations idempotently, so an
    // existing chorus.db gets schema upgrades as part of setup.
    let db_path = data_subdir.join("chorus.db");
    let _ = Store::open(db_path.to_str().unwrap())?;

    // Persist config — machine_id (stable across re-runs) + template_dir,
    // so `chorus run` can read the chosen paths without the user re-passing
    // --template-dir every time.
    let mut cfg = ChorusConfig::load(&data_dir)?.unwrap_or_default();
    let machine_id = cfg.ensure_machine_id().to_string();
    cfg.agent_template.dir = Some(template_dir_raw.clone());

    // Pin runtime binaries to the exact paths detected on this machine,
    // but don't overwrite anything the user has already customized. When
    // a CLI binary shows up in multiple PATH entries (e.g. ~/.local/bin
    // AND /usr/local/bin), prompt interactively. ACP adapters always use
    // the first match — they're less likely to ship multiple versions.
    fill_binary_path(&mut cfg.claude.binary_path, "claude", interactive);
    fill_resolved_path(&mut cfg.claude.acp_adaptor, "claude-agent-acp");
    fill_binary_path(&mut cfg.codex.binary_path, "codex", interactive);
    fill_resolved_path(&mut cfg.codex.acp_adaptor, "codex-acp");
    fill_binary_path(&mut cfg.kimi.binary_path, "kimi", interactive);
    fill_binary_path(&mut cfg.opencode.binary_path, "opencode", interactive);

    let cfg_path = cfg.save(&data_dir)?;

    section("Layout");
    row_ok("config", &format!("wrote {}", cfg_path.display()));
    row_ok("data", &format!("{}", data_subdir.display()));
    row_ok("logs", &format!("{}", logs_dir.display()));
    row_ok("agents", &format!("{}", agents_dir.display()));
    row_ok(
        "machine id",
        &format!("{} (persistent)", style(&machine_id).cyan()),
    );

    // Summary line
    println!();
    if detected_runtimes.is_empty() {
        println!(
            "  {} no agent runtimes detected · install one, then re-run setup",
            style(WARN).yellow()
        );
    } else {
        println!(
            "  {} runtimes available: {}",
            style("→").cyan().bold(),
            style(detected_runtimes.join(", ")).bold()
        );
        println!(
            "  {} {}",
            style(" ").dim(),
            style("chorus agent create <name> --runtime <runtime>").dim()
        );
    }

    footer(started.elapsed(), "chorus run");
    Ok(())
}

async fn cmd_run(
    port: u16,
    data_dir: Option<String>,
    no_open: bool,
    template_dir: Option<String>,
) -> anyhow::Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let template_dir = resolve_template_dir(&data_dir_str, template_dir);

    if !no_open {
        let url = format!("http://localhost:{port}");
        tokio::spawn(async move {
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(400))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(err = %e, "failed to build health-probe client; skipping browser open");
                    return;
                }
            };
            let health = format!("{url}/health");
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(res) = client.get(&health).send().await {
                    if res.status().is_success() {
                        if let Err(e) = open::that(&url) {
                            tracing::warn!(url = %url, err = %e, "could not open browser");
                        }
                        return;
                    }
                }
            }
            tracing::warn!(
                "server did not respond to /health within budget; skipping browser open"
            );
        });
    }

    serve(port, data_dir_str, template_dir).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_tool_returns_none_for_missing_binary() {
        assert!(check_tool("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn extract_version_handles_common_formats() {
        assert_eq!(extract_version("bun 1.3.12"), Some("1.3.12".to_string()));
        assert_eq!(
            extract_version("kimi, version 1.31.0"),
            Some("1.31.0".to_string())
        );
        assert_eq!(
            extract_version("codex-cli 0.120.0"),
            Some("0.120.0".to_string())
        );
        assert_eq!(extract_version("no version here"), None);
    }

    #[test]
    fn which_all_finds_every_match_across_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        let dir_c = tmp.path().join("c");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&dir_c).unwrap();
        // Two copies of a fake binary, different dirs.
        for d in [&dir_a, &dir_c] {
            let p = d.join("myfake-bin");
            std::fs::write(&p, "#!/bin/sh\ntrue\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&p).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&p, perms).unwrap();
            }
        }

        let joined = std::env::join_paths([&dir_a, &dir_b, &dir_c]).unwrap();
        let prev = std::env::var_os("PATH");
        // SAFETY: env mutation is not thread-safe, but cargo test runs
        // each test in its own thread; callers treat PATH as read-only.
        std::env::set_var("PATH", &joined);
        let found = which_all("myfake-bin");
        if let Some(p) = prev {
            std::env::set_var("PATH", p);
        }
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], dir_a.join("myfake-bin"));
        assert_eq!(found[1], dir_c.join("myfake-bin"));
    }
}
