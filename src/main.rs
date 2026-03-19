use clap::{Parser, Subcommand};
use std::sync::Arc;

mod models;
mod store;
mod server;
mod agent_manager;
mod drivers;
mod bridge;

use models::*;
use store::Store;
use server::build_router;
use agent_manager::AgentManager;

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
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Create and start a new agent
    Create {
        name: String,
        #[arg(long, default_value = "claude")]
        runtime: String,
        #[arg(long, default_value = "sonnet")]
        model: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Stop a running agent
    Stop {
        name: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Bridge { agent_id, server_url }) => {
            bridge::run_bridge(agent_id, server_url).await
        }

        Some(Commands::Serve { port, data_dir }) => {
            let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
            serve(port, data_dir_str).await
        }

        None => {
            serve(3001, default_data_dir()).await
        }

        Some(Commands::Send { target, content, server_url }) => {
            let username = whoami::username();
            let client = reqwest::Client::new();
            let res = client.post(format!("{server_url}/internal/agent/{username}/send"))
                .json(&serde_json::json!({ "target": target, "content": content }))
                .send()
                .await?;
            let data: serde_json::Value = res.json().await?;
            if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
                eprintln!("Error: {err}");
            } else {
                let msg_id = data.get("messageId").and_then(|v| v.as_str()).unwrap_or("-");
                println!("Message sent to {target}. ID: {msg_id}");
            }
            Ok(())
        }

        Some(Commands::History { channel, limit, server_url }) => {
            let username = whoami::username();
            let client = reqwest::Client::new();
            let res = client.get(format!(
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
            let res = client.get(format!("{server_url}/internal/agent/{username}/server"))
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

        Some(Commands::Channel { name, description, server_url: _ }) => {
            let username = whoami::username();
            let data_dir = default_data_dir();
            let db_path = format!("{data_dir}/chorus.db");
            let store = Store::open(&db_path)?;
            store.create_channel(&name, description.as_deref(), ChannelType::Channel)?;
            store.join_channel(&name, &username, SenderType::Human)?;
            println!("Channel #{name} created.");
            Ok(())
        }

        Some(Commands::Agent { cmd }) => {
            match cmd {
                AgentCommands::Create { name, runtime, model, description, server_url: _ } => {
                    let data_dir = default_data_dir();
                    let db_path = format!("{data_dir}/chorus.db");
                    let store = Store::open(&db_path)?;
                    store.create_agent_record(&name, &name, description.as_deref(), &runtime, &model)?;
                    // Join default channels
                    for ch in store.list_channels()? {
                        let _ = store.join_channel(&ch.name, &name, SenderType::Agent);
                    }
                    println!("Agent @{name} created (runtime: {runtime}, model: {model}).");
                    println!("Start it by running the server: `chorus serve`");
                    Ok(())
                }
                AgentCommands::Stop { name, server_url: _ } => {
                    println!("Stopping agent @{name}...");
                    let data_dir = default_data_dir();
                    let db_path = format!("{data_dir}/chorus.db");
                    let store = Store::open(&db_path)?;
                    store.update_agent_status(&name, AgentStatus::Inactive)?;
                    println!("Agent @{name} marked as inactive.");
                    Ok(())
                }
                AgentCommands::List { server_url } => {
                    let client = reqwest::Client::new();
                    let username = whoami::username();
                    let res = client.get(format!("{server_url}/internal/agent/{username}/server"))
                        .send()
                        .await?;
                    let data: serde_json::Value = res.json().await?;
                    if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
                        if agents.is_empty() {
                            println!("No agents.");
                        } else {
                            for a in agents {
                                let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                                let runtime = a.get("runtime").and_then(|v| v.as_str()).unwrap_or("?");
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

async fn serve(port: u16, data_dir_str: String) -> anyhow::Result<()> {
    let data_dir = std::path::PathBuf::from(&data_dir_str);
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap())?);

    // Default human = OS username
    let username = whoami::username();
    let _ = store.add_human(&username);

    // Create default #general channel if none exist
    if store.list_channels()?.is_empty() {
        store.create_channel("general", Some("General channel for all members"), ChannelType::Channel)?;
        store.join_channel("general", &username, SenderType::Human)?;
    }

    let server_url = format!("http://localhost:{port}");
    let bridge_binary = std::env::current_exe()?.to_string_lossy().into_owned();
    let _manager = Arc::new(AgentManager::new(
        store.clone(),
        data_dir.join("agents"),
        bridge_binary,
        server_url.clone(),
    ));

    let router = build_router(store.clone());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("Chorus running at {server_url}");
    println!("Human user: @{username}");
    println!("Use `chorus send '#general' 'hello'` to send messages");
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
