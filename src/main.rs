use clap::{Parser, Subcommand};
use std::sync::Arc;

use chorus::agent::manager::AgentManager;
use chorus::agent::AgentRuntime;
use chorus::bridge;
use chorus::store::agents::AgentStatus;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::AgentRecordUpsert;
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
                tracing::error!("Error: {err}");
            } else {
                let msg_id = data
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                tracing::info!("Message sent to {target}. ID: {msg_id}");
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
                tracing::error!("Error: {err}");
            } else if let Some(messages) = data.get("messages").and_then(|v| v.as_array()) {
                if messages.is_empty() {
                    tracing::info!("No messages.");
                } else {
                    for m in messages {
                        let sender = m.get("senderName").and_then(|v| v.as_str()).unwrap_or("?");
                        let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let time = m.get("createdAt").and_then(|v| v.as_str()).unwrap_or("");
                        tracing::info!("[{time}] @{sender}: {content}");
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
            tracing::info!("== Channels ==");
            if let Some(channels) = data.get("channels").and_then(|v| v.as_array()) {
                for ch in channels {
                    let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let joined = ch.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
                    let desc = ch.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let status = if joined { "joined" } else { "not joined" };
                    if desc.is_empty() {
                        tracing::info!("  #{name} [{status}]");
                    } else {
                        tracing::info!("  #{name} [{status}] — {desc}");
                    }
                }
            }
            tracing::info!("\n== Agents ==");
            if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
                for a in agents {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                    tracing::info!("  @{name} ({status})");
                }
            }
            tracing::info!("\n== Humans ==");
            if let Some(humans) = data.get("humans").and_then(|v| v.as_array()) {
                for h in humans {
                    let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    tracing::info!("  @{name}");
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
            tracing::info!("Channel #{name} created.");
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
                    store.create_agent_record(&AgentRecordUpsert {
                        name: &name,
                        display_name: &name,
                        description: description.as_deref(),
                        system_prompt: None,
                        runtime: &runtime,
                        model: &model,
                        reasoning_effort: None,
                        env_vars: &[],
                    })?;
                    // Join default channels
                    for ch in store.get_channels()? {
                        let _ = store.join_channel(&ch.name, &name, SenderType::Agent);
                    }
                    tracing::info!("Agent @{name} created (runtime: {runtime}, model: {model}).");
                    tracing::info!("Start it by running the server: `chorus serve`");
                    Ok(())
                }
                AgentCommands::Stop { name, data_dir } => {
                    tracing::info!("Stopping agent @{name}...");
                    let data_dir = data_dir.unwrap_or_else(default_data_dir);
                    let db_path = format!("{data_dir}/chorus.db");
                    let store = Store::open(&db_path)?;
                    store.update_agent_status(&name, AgentStatus::Inactive)?;
                    tracing::info!("Agent @{name} marked as inactive.");
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
                            tracing::info!("No agents.");
                        } else {
                            for a in agents {
                                let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let status =
                                    a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                                let runtime =
                                    a.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
                                tracing::info!("  @{name} [{status}] (runtime: {runtime})");
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
            tracing::warn!(
                "Warning: {} agent(s) failed to auto-restart and were marked inactive: {}",
                failed_agents.len(),
                failed_agents
                    .iter()
                    .map(|(agent_name, _)| agent_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for (agent_name, error_detail) in &failed_agents {
                tracing::warn!("  - {agent_name}: {error_detail}");
            }
            tracing::warn!("They will be retried on next message delivery. To restart immediately: `chorus agent start <name>`");
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
    tracing::info!("Chorus running at {server_url}");
    tracing::info!("Human user: @{username}");
    tracing::info!("Use `chorus send '#all' 'hello'` to send messages");
    tracing::info!("Use `chorus agent create <name>` to create an agent");

    // Graceful shutdown on Ctrl+C
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("\nShutting down...");
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
