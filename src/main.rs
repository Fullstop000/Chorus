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
        #[arg(long, env = "CHORUS_TEMPLATE_DIR", default_value = "~/agency-agents")]
        template_dir: String,
    },
    /// First-run setup: check environment, initialize data dir, build UI
    Setup {
        /// Non-interactive mode (skip prompts, accept defaults)
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        data_dir: Option<String>,
        /// Rebuild ui/dist even if it already has content
        #[arg(long)]
        force_ui_build: bool,
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
        #[arg(long, env = "CHORUS_TEMPLATE_DIR", default_value = "~/agency-agents")]
        template_dir: String,
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

    // Initialize tracing. Default level: chorus=info (override with RUST_LOG).
    // Enable RUST_BACKTRACE by default so Backtrace::capture() works in error handlers.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("chorus=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .init();

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
            serve(port, data_dir_str, template_dir).await
        }

        Some(Commands::Setup {
            yes,
            data_dir,
            force_ui_build,
        }) => cmd_setup(yes, data_dir, force_ui_build).await,

        Some(Commands::Run {
            port,
            data_dir,
            no_open,
            template_dir,
        }) => cmd_run(port, data_dir, no_open, template_dir).await,

        None => serve(3001, default_data_dir(), "~/agency-agents".to_string()).await,

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
            let db_path = format!("{data_dir}/chorus.db");
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
                    let db_path = format!("{data_dir}/chorus.db");
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
                    let db_path = format!("{data_dir}/chorus.db");
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
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap())?);

    // Default human = OS username
    let username = whoami::username();
    let _ = store.create_human(&username);

    // Ensure built-in system channels exist and upgrade legacy installs to #all.
    store.ensure_builtin_channels(&username)?;

    let server_url = format!("http://localhost:{port}");
    let bridge_binary = std::env::current_exe()?.to_string_lossy().into_owned();
    let manager = Arc::new(AgentManager::new(
        store.clone(),
        data_dir.join("agents"),
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

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

/// Detected CLI tool: name, install hint, version if found.
struct ToolReport {
    name: &'static str,
    hint: &'static str,
    version: Option<String>,
}

fn check_tool(name: &'static str, hint: &'static str) -> ToolReport {
    let version = Command::new(name)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty());
    ToolReport {
        name,
        hint,
        version,
    }
}

fn print_tool(report: &ToolReport, required: bool) {
    match &report.version {
        Some(v) => println!("  [ok] {:<10} {}", report.name, v),
        None => {
            let tag = if required { "MISSING" } else { "missing" };
            println!("  [{}] {:<10} install: {}", tag, report.name, report.hint);
        }
    }
}

async fn cmd_setup(
    yes: bool,
    data_dir: Option<String>,
    force_ui_build: bool,
) -> anyhow::Result<()> {
    let interactive = !yes && std::io::stdin().is_terminal();
    if !yes && !interactive {
        println!("stdin is not a terminal; running in --yes mode.");
    }

    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let data_dir = PathBuf::from(&data_dir_str);

    println!("\nChorus setup");
    println!("============");
    println!("Data directory: {}", data_dir.display());

    // 1. Environment checks
    println!("\nEnvironment:");
    let bun = check_tool("bun", "https://bun.sh");
    print_tool(&bun, true);
    let runtimes = [
        check_tool("claude", "https://docs.claude.com/en/docs/claude-code"),
        check_tool("codex", "https://github.com/openai/codex"),
        check_tool("kimi", "https://github.com/MoonshotAI/kimi-cli"),
        check_tool("opencode", "https://opencode.ai"),
    ];
    for r in &runtimes {
        print_tool(r, false);
    }
    let detected_runtimes: Vec<&str> = runtimes
        .iter()
        .filter(|r| r.version.is_some())
        .map(|r| r.name)
        .collect();
    if detected_runtimes.is_empty() {
        println!("\n  Note: no agent runtimes detected. You can install one later and restart.");
    }

    // 2. Data dir + DB
    println!("\nData directory:");
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("chorus.db");
    if db_path.exists() {
        println!("  [ok] already initialized at {}", data_dir.display());
    } else {
        let _ = Store::open(db_path.to_str().unwrap())?;
        println!("  [ok] initialized SQLite database");
    }

    // 3. UI build
    println!("\nUI assets:");
    let ui_dir = PathBuf::from("ui");
    let dist = ui_dir.join("dist");
    let has_real_dist = dist.join("assets").exists() || dist.join("index.js").exists();
    if bun.version.is_none() {
        println!("  [skip] bun not installed; binary will serve embedded placeholder UI.");
    } else if !ui_dir.join("package.json").exists() {
        println!("  [skip] ui/package.json not found (running from installed binary?).");
    } else if has_real_dist && !force_ui_build {
        println!("  [ok] ui/dist already built (use --force-ui-build to rebuild)");
    } else {
        println!("  building UI (bun install && bun run build)...");
        run_cmd(Command::new("bun").arg("install").current_dir(&ui_dir))?;
        run_cmd(
            Command::new("bun")
                .args(["run", "build"])
                .current_dir(&ui_dir),
        )?;
        println!("  [ok] ui/dist built");
    }

    // 4. Interactive seed
    if interactive {
        use dialoguer::Confirm;
        let seed = Confirm::new()
            .with_prompt("Create a welcome channel (#general)?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if seed {
            let store = Store::open(db_path.to_str().unwrap())?;
            let username = whoami::username();
            match store.create_channel("general", Some("General chat"), ChannelType::Channel) {
                Ok(_) => {
                    let _ = store.join_channel("general", &username, SenderType::Human);
                    println!("  [ok] #general created");
                }
                Err(e) => println!("  [skip] could not create channel: {e}"),
            }
        }
    }

    println!("\nSetup complete. Next: `chorus run` to start the server and open the UI.");
    Ok(())
}

fn run_cmd(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd.status().map_err(|e| {
        anyhow::anyhow!(
            "failed to run `{}`: {e}",
            cmd.get_program().to_string_lossy()
        )
    })?;
    if !status.success() {
        anyhow::bail!(
            "`{}` exited with status {}",
            cmd.get_program().to_string_lossy(),
            status
        );
    }
    Ok(())
}

async fn cmd_run(
    port: u16,
    data_dir: Option<String>,
    no_open: bool,
    template_dir: String,
) -> anyhow::Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);

    if !no_open {
        let url = format!("http://localhost:{port}");
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(400))
                .build()
                .unwrap();
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
        let r = check_tool("definitely-not-a-real-binary-xyzzy", "n/a");
        assert!(r.version.is_none());
    }

    #[test]
    fn check_tool_captures_version_of_common_binary() {
        // `sh --version` exists on every POSIX host the CI runs on.
        let r = check_tool("sh", "n/a");
        // On some minimal BusyBox shells `sh --version` returns non-zero.
        // So we only assert: if the command succeeded, we got a non-empty line.
        if let Some(v) = r.version {
            assert!(!v.is_empty());
        }
    }
}
