use crate::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::models::*;
use crate::store::Store;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use tokio::sync::Mutex;

struct RunningAgent {
    process: Child,
    driver: Arc<dyn Driver>,
    session_id: Option<String>,
    is_in_receive_message: bool,
    pending_notification_count: u32,
}

pub struct AgentManager {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    store: Arc<Store>,
    data_dir: PathBuf,
    bridge_binary: String,
    server_url: String,
}

fn get_driver(runtime: &str) -> anyhow::Result<Arc<dyn Driver>> {
    match runtime {
        "claude" => Ok(Arc::new(crate::drivers::claude::ClaudeDriver)),
        "codex" => Ok(Arc::new(crate::drivers::codex::CodexDriver)),
        _ => anyhow::bail!("Unknown runtime: {runtime}"),
    }
}

impl AgentManager {
    pub fn new(
        store: Arc<Store>,
        data_dir: PathBuf,
        bridge_binary: String,
        server_url: String,
    ) -> Self {
        Self {
            agents: Arc::new(Mutex::new(HashMap::new())),
            store,
            data_dir,
            bridge_binary,
            server_url,
        }
    }

    /// Start an agent process. Creates workspace dir, writes MEMORY.md, spawns CLI.
    pub async fn start_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        // Check if already running
        {
            let agents = self.agents.lock().await;
            if agents.contains_key(agent_name) {
                return Ok(());
            }
        }

        // Look up agent from store
        let agent = self
            .store
            .get_agent(agent_name)?
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {agent_name}"))?;

        let driver = get_driver(&agent.runtime)?;

        // Build AgentConfig
        let config = AgentConfig {
            name: agent.name.clone(),
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            session_id: agent.session_id.clone(),
            env_vars: None,
        };

        // Create workspace directory
        let agent_data_dir = self.data_dir.join(agent_name);
        tokio::fs::create_dir_all(&agent_data_dir).await?;

        // Write initial MEMORY.md if absent
        let memory_md_path = agent_data_dir.join("MEMORY.md");
        if !memory_md_path.exists() {
            let agent_display = &config.display_name;
            let description = config.description.as_deref().unwrap_or("No role defined yet.");
            let initial_memory = format!(
                "# {agent_display}\n\n\
                 ## Role\n\
                 {description}\n\n\
                 ## Key Knowledge\n\
                 - No notes yet.\n\n\
                 ## Active Context\n\
                 - First startup.\n"
            );
            tokio::fs::write(&memory_md_path, initial_memory).await?;
        }

        // Create notes directory
        tokio::fs::create_dir_all(agent_data_dir.join("notes")).await?;

        // Determine if this is a resume
        let is_resume = agent.session_id.is_some();

        // Get unread summary
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        // Build the initial prompt
        let prompt = if !is_resume {
            // Fresh start
            driver.build_system_prompt(&config, &agent.id)
        } else if !unread_summary.is_empty() {
            // Resume with unread messages
            let mut prompt = String::from("You have unread messages from while you were offline:");
            for (ch, count) in &unread_summary {
                prompt.push_str(&format!("\n- {ch}: {count} unread"));
            }
            prompt.push_str(
                "\n\nUse read_history to catch up on important channels, \
                 then call receive_message(block=true) to listen for new messages.",
            );
            if driver.supports_stdin_notification() {
                prompt.push_str(
                    "\n\nNote: While you are busy, you may receive \
                     [System notification: ...] messages. \
                     Finish your current step, then call receive_message to check.",
                );
            }
            prompt
        } else {
            // Resume with no unread
            let prefix = driver.mcp_tool_prefix();
            let mut prompt = format!(
                "No new messages while you were away. \
                 Call {prefix}receive_message(block=true) to listen for new messages."
            );
            if driver.supports_stdin_notification() {
                prompt.push_str(
                    "\n\nNote: While you are busy, you may receive \
                     [System notification: ...] messages about new messages. \
                     Finish your current step, then call receive_message to check.",
                );
            }
            prompt
        };

        // Spawn the agent process
        let ctx = SpawnContext {
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            config,
            prompt,
            working_directory: agent_data_dir.to_string_lossy().into_owned(),
            bridge_binary: self.bridge_binary.clone(),
            server_url: self.server_url.clone(),
        };

        let mut child = driver.spawn(&ctx)?;

        // Take stdout for reading
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture agent stdout"))?;

        let session_id = agent.session_id.clone();

        // Insert into running agents map
        {
            let mut agents = self.agents.lock().await;
            agents.insert(
                agent_name.to_string(),
                RunningAgent {
                    process: child,
                    driver: driver.clone(),
                    session_id,
                    is_in_receive_message: false,
                    pending_notification_count: 0,
                },
            );
        }

        // Update status to active
        self.store
            .update_agent_status(agent_name, AgentStatus::Active)?;

        // Spawn stdout reader task
        self.spawn_output_reader(agent_name.to_string(), stdout, driver);

        Ok(())
    }

    /// Stop an agent process.
    pub async fn stop_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut running = {
            let mut agents = self.agents.lock().await;
            match agents.remove(agent_name) {
                Some(r) => r,
                None => return Ok(()),
            }
        };

        // Kill the process
        let _ = running.process.kill();

        // Update status to inactive
        self.store
            .update_agent_status(agent_name, AgentStatus::Inactive)?;

        Ok(())
    }

    /// Sleep an agent (kill process, keep status as sleeping).
    pub async fn sleep_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut running = {
            let mut agents = self.agents.lock().await;
            match agents.remove(agent_name) {
                Some(r) => r,
                None => return Ok(()),
            }
        };

        eprintln!("[Agent {agent_name}] Hibernating (sleeping)");

        // Kill the process
        let _ = running.process.kill();

        // Status stays as sleeping — set explicitly
        self.store
            .update_agent_status(agent_name, AgentStatus::Sleeping)?;

        Ok(())
    }

    /// Deliver a message notification to agent stdin.
    pub async fn notify_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut agents = self.agents.lock().await;
        let running = match agents.get_mut(agent_name) {
            Some(r) => r,
            None => return Ok(()),
        };

        if !running.driver.supports_stdin_notification() {
            return Ok(());
        }
        if running.is_in_receive_message {
            return Ok(());
        }
        if running.session_id.is_none() {
            return Ok(());
        }

        running.pending_notification_count += 1;
        let count = running.pending_notification_count;

        // Spawn a delayed notification task
        let agents_ref = self.agents.clone();
        let name = agent_name.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let mut agents = agents_ref.lock().await;
            if let Some(running) = agents.get_mut(&name) {
                let current_count = running.pending_notification_count;
                if current_count == 0 {
                    return;
                }
                if running.is_in_receive_message {
                    running.pending_notification_count = 0;
                    return;
                }
                let sid = match &running.session_id {
                    Some(s) => s.clone(),
                    None => return,
                };

                // Only send if count hasn't changed (i.e., this is the latest timer)
                if current_count != count {
                    return;
                }

                running.pending_notification_count = 0;

                let plural = if current_count > 1 { "s" } else { "" };
                let them = if current_count > 1 { "them" } else { "it" };
                let notification = format!(
                    "[System notification: You have {current_count} new message{plural} waiting. \
                     Call receive_message to read {them} when you're ready.]"
                );

                eprintln!(
                    "[Agent {name}] Sending stdin notification: {current_count} message(s)"
                );

                if let Some(encoded) = running.driver.encode_stdin_message(&notification, &sid) {
                    if let Some(stdin) = running.process.stdin.as_mut() {
                        let _ = writeln!(stdin, "{encoded}");
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop all running agents.
    pub async fn stop_all(&self) -> anyhow::Result<()> {
        let names: Vec<String> = {
            let agents = self.agents.lock().await;
            agents.keys().cloned().collect()
        };
        for name in names {
            self.stop_agent(&name).await?;
        }
        Ok(())
    }

    pub async fn get_running_agent_names(&self) -> Vec<String> {
        let agents = self.agents.lock().await;
        agents.keys().cloned().collect()
    }

    /// Spawn a background task that reads stdout lines and processes events.
    fn spawn_output_reader(
        &self,
        agent_name: String,
        stdout: std::process::ChildStdout,
        driver: Arc<dyn Driver>,
    ) {
        let agents = self.agents.clone();
        let store = self.store.clone();
        let name = agent_name.clone();

        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("[Agent {name}] stdout read error: {e}");
                        break;
                    }
                };

                if line.trim().is_empty() {
                    continue;
                }

                let events = driver.parse_line(&line);
                for event in events {
                    // We need to block on the async lock
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        Self::handle_parsed_event(&agents, &store, &name, event, &driver).await;
                    });
                }
            }

            // Process exited — stdout closed
            eprintln!("[Agent {name}] stdout reader ended, checking exit status");

            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut agents_map = agents.lock().await;
                if let Some(mut running) = agents_map.remove(&name) {
                    // Wait for exit code
                    match running.process.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            eprintln!("[Agent {name}] Process exited with code {code}");
                            if code == 0 {
                                let _ =
                                    store.update_agent_status(&name, AgentStatus::Sleeping);
                            } else {
                                eprintln!(
                                    "[Agent {name}] Process crashed (exit code {code}) — marking inactive"
                                );
                                let _ =
                                    store.update_agent_status(&name, AgentStatus::Inactive);
                            }
                        }
                        Err(e) => {
                            eprintln!("[Agent {name}] Failed to get exit status: {e}");
                            let _ =
                                store.update_agent_status(&name, AgentStatus::Inactive);
                        }
                    }
                }
            });
        });
    }

    /// Handle a single parsed event from an agent's stdout.
    async fn handle_parsed_event(
        agents: &Arc<Mutex<HashMap<String, RunningAgent>>>,
        store: &Arc<Store>,
        agent_name: &str,
        event: ParsedEvent,
        driver: &Arc<dyn Driver>,
    ) {
        let mut agents_map = agents.lock().await;
        let running = match agents_map.get_mut(agent_name) {
            Some(r) => r,
            None => return,
        };

        match event {
            ParsedEvent::SessionInit { session_id } => {
                running.session_id = Some(session_id.clone());
                let _ = store.update_agent_session(agent_name, Some(&session_id));
            }
            ParsedEvent::ToolCall { ref name, .. } => {
                let receive_tool = format!("{}receive_message", driver.mcp_tool_prefix());
                if *name == receive_tool {
                    running.is_in_receive_message = true;
                    running.pending_notification_count = 0;
                } else {
                    running.is_in_receive_message = false;
                }
            }
            ParsedEvent::Thinking { .. } | ParsedEvent::Text { .. } => {
                running.is_in_receive_message = false;
            }
            ParsedEvent::TurnEnd { session_id } => {
                running.is_in_receive_message = false;
                if let Some(ref sid) = session_id {
                    running.session_id = Some(sid.clone());
                    let _ = store.update_agent_session(agent_name, Some(sid));
                }
            }
            ParsedEvent::Error { ref message } => {
                eprintln!("[Agent {agent_name}] Error event: {message}");
            }
        }
    }
}
