use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;

use tracing::{debug, error, info, warn};
use tokio::sync::Mutex;

use crate::activity_log::{self, ActivityLogMap};
use crate::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::models::*;
use crate::server::AgentLifecycle;
use crate::store::Store;

struct RunningAgent {
    process: Child,
    driver: Arc<dyn Driver>,
    session_id: Option<String>,
    is_in_receive_message: bool,
    pending_notification_count: u32,
}

pub struct AgentManager {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    activity_logs: Arc<ActivityLogMap>,
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
            activity_logs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            store,
            data_dir,
            bridge_binary,
            server_url,
        }
    }

    /// Start an agent process. Creates workspace dir, writes MEMORY.md, spawns CLI.
    pub async fn start_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        {
            let agents = self.agents.lock().await;
            if agents.contains_key(agent_name) {
                return Ok(());
            }
        }

        let agent = self
            .store
            .get_agent(agent_name)?
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {agent_name}"))?;

        let driver = get_driver(&agent.runtime)?;
        let is_codex_driver = driver.id() == "codex";

        let config = AgentConfig {
            name: agent.name.clone(),
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            session_id: agent.session_id.clone(),
            env_vars: None,
        };

        let agent_data_dir = self.data_dir.join(agent_name);
        tokio::fs::create_dir_all(&agent_data_dir).await?;

        let memory_md_path = agent_data_dir.join("MEMORY.md");
        if !memory_md_path.exists() {
            let description = config.description.as_deref().unwrap_or("No role defined yet.");
            tokio::fs::write(
                &memory_md_path,
                format!(
                    "# {}\n\n## Role\n{}\n\n## Key Knowledge\n- No notes yet.\n\n## Active Context\n- First startup.\n",
                    config.display_name, description
                ),
            ).await?;
        }
        tokio::fs::create_dir_all(agent_data_dir.join("notes")).await?;

        let is_resume = agent.session_id.is_some();
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        let prompt = build_start_prompt(&config, driver.as_ref(), is_codex_driver, is_resume, &unread_summary);

        let ctx = SpawnContext {
            agent_id: agent.name.clone(),
            agent_name: agent.name.clone(),
            config,
            prompt,
            working_directory: agent_data_dir.to_string_lossy().into_owned(),
            bridge_binary: self.bridge_binary.clone(),
            server_url: self.server_url.clone(),
        };

        let mut child = driver.spawn(&ctx)?;
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture agent stdout"))?;

        {
            let mut agents = self.agents.lock().await;
            agents.insert(agent_name.to_string(), RunningAgent {
                process: child,
                driver: driver.clone(),
                session_id: agent.session_id.clone(),
                is_in_receive_message: false,
                pending_notification_count: 0,
            });
        }

        self.store.update_agent_status(agent_name, AgentStatus::Active)?;
        activity_log::set_activity_state(&self.activity_logs, agent_name, "working", "Starting…");

        self.spawn_output_reader(agent_name.to_string(), stdout, driver);
        Ok(())
    }

    /// Stop an agent process and mark it inactive.
    pub async fn stop_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut running = {
            let mut agents = self.agents.lock().await;
            match agents.remove(agent_name) {
                Some(r) => r,
                None => return Ok(()),
            }
        };
        let _ = running.process.kill();
        self.store.update_agent_status(agent_name, AgentStatus::Inactive)?;
        Ok(())
    }

    /// Kill process but keep status as sleeping (will auto-restart on next message).
    pub async fn sleep_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut running = {
            let mut agents = self.agents.lock().await;
            match agents.remove(agent_name) {
                Some(r) => r,
                None => return Ok(()),
            }
        };
        info!(agent = %agent_name, "hibernating (sleeping)");
        let _ = running.process.kill();
        self.store.update_agent_status(agent_name, AgentStatus::Sleeping)?;
        Ok(())
    }

    /// Deliver a wakeup notification to agent stdin.
    pub async fn notify_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut agents = self.agents.lock().await;
        let running = match agents.get_mut(agent_name) {
            Some(r) => r,
            None => return Ok(()),
        };

        if !running.driver.supports_stdin_notification()
            || running.is_in_receive_message
            || running.session_id.is_none()
        {
            return Ok(());
        }

        running.pending_notification_count += 1;
        let count = running.pending_notification_count;

        let agents_ref = self.agents.clone();
        let name = agent_name.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let mut agents = agents_ref.lock().await;
            if let Some(running) = agents.get_mut(&name) {
                let current_count = running.pending_notification_count;
                if current_count == 0 || running.is_in_receive_message || current_count != count {
                    running.pending_notification_count = 0;
                    return;
                }
                let sid = match running.session_id.clone() {
                    Some(s) => s,
                    None => return,
                };
                running.pending_notification_count = 0;

                let plural = if current_count > 1 { "s" } else { "" };
                let them = if current_count > 1 { "them" } else { "it" };
                let notification = format!(
                    "[System notification: You have {current_count} new message{plural} waiting. \
                     Call receive_message to read {them} when you're ready.]"
                );
                info!(agent = %name, count = current_count, "sending stdin notification");
                if let Some(encoded) = running.driver.encode_stdin_message(&notification, &sid) {
                    if let Some(stdin) = running.process.stdin.as_mut() {
                        let _ = writeln!(stdin, "{encoded}");
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn stop_all(&self) -> anyhow::Result<()> {
        let names: Vec<String> = self.agents.lock().await.keys().cloned().collect();
        for name in names {
            self.stop_agent(&name).await?;
        }
        Ok(())
    }

    pub async fn get_running_agent_names(&self) -> Vec<String> {
        self.agents.lock().await.keys().cloned().collect()
    }

    fn spawn_output_reader(
        &self,
        agent_name: String,
        stdout: std::process::ChildStdout,
        driver: Arc<dyn Driver>,
    ) {
        let agents = self.agents.clone();
        let activity_logs = self.activity_logs.clone();
        let store = self.store.clone();
        let name = agent_name.clone();

        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        error!(agent = %name, err = %e, "stdout read error");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }

                for event in driver.parse_line(&line) {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        handle_parsed_event(&agents, &activity_logs, &store, &name, event, &driver).await;
                    });
                }
            }

            info!(agent = %name, "stdout reader ended — checking exit status");
            activity_log::set_activity_state(&activity_logs, &name, "offline", "Process stopped");

            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut agents_map = agents.lock().await;
                if let Some(mut running) = agents_map.remove(&name) {
                    match running.process.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            if code == 0 {
                                info!(agent = %name, code, "process exited cleanly — sleeping");
                                let _ = store.update_agent_status(&name, AgentStatus::Sleeping);
                            } else {
                                warn!(agent = %name, code, "process crashed — marking inactive");
                                let _ = store.update_agent_status(&name, AgentStatus::Inactive);
                            }
                        }
                        Err(e) => {
                            error!(agent = %name, err = %e, "failed to get exit status");
                            let _ = store.update_agent_status(&name, AgentStatus::Inactive);
                        }
                    }
                }
            });
        });
    }
}

async fn handle_parsed_event(
    agents: &Arc<Mutex<HashMap<String, RunningAgent>>>,
    logs: &Arc<ActivityLogMap>,
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
            info!(agent = %agent_name, session = %session_id, "session started");
            running.session_id = Some(session_id.clone());
            let _ = store.update_agent_session(agent_name, Some(&session_id));
            activity_log::set_activity_state(logs, agent_name, "online", "Ready");
        }
        ParsedEvent::Thinking { ref text } => {
            running.is_in_receive_message = false;
            let preview: String = text.chars().take(120).collect();
            let preview = if text.chars().count() > 120 { format!("{preview}…") } else { preview };
            debug!(agent = %agent_name, text = %preview, "thinking");
            activity_log::push_activity(logs, agent_name, ActivityEntry::Thinking { text: text.clone() });
            activity_log::set_activity_state(logs, agent_name, "thinking", "Thinking…");
        }
        ParsedEvent::Text { ref text } => {
            running.is_in_receive_message = false;
            let preview: String = text.chars().take(120).collect();
            let preview = if text.chars().count() > 120 { format!("{preview}…") } else { preview };
            info!(agent = %agent_name, text = %preview, "text output");
            activity_log::push_activity(logs, agent_name, ActivityEntry::Text { text: text.clone() });
        }
        ParsedEvent::ToolCall { ref name, ref input } => {
            let receive_tool = format!("{}receive_message", driver.mcp_tool_prefix());
            if *name == receive_tool {
                running.is_in_receive_message = true;
                running.pending_notification_count = 0;
                info!(agent = %agent_name, "waiting for messages");
                activity_log::push_activity(logs, agent_name, ActivityEntry::ToolStart {
                    tool_name: name.clone(),
                    tool_input: String::new(),
                });
                activity_log::set_activity_state(logs, agent_name, "online", "Waiting for messages");
            } else {
                running.is_in_receive_message = false;
                let display_name = driver.tool_display_name(name);
                let tool_input = driver.summarize_tool_input(name, input);
                info!(agent = %agent_name, tool = %name, input = %tool_input, "tool call");
                activity_log::push_activity(logs, agent_name, ActivityEntry::ToolStart {
                    tool_name: display_name.clone(),
                    tool_input,
                });
                activity_log::set_activity_state(logs, agent_name, "working", &display_name);
            }
        }
        ParsedEvent::TurnEnd { session_id } => {
            info!(agent = %agent_name, "turn ended");
            running.is_in_receive_message = false;
            if let Some(ref sid) = session_id {
                running.session_id = Some(sid.clone());
                let _ = store.update_agent_session(agent_name, Some(sid));
            }
            activity_log::set_activity_state(logs, agent_name, "online", "Idle");
        }
        ParsedEvent::Error { ref message } => {
            error!(agent = %agent_name, message = %message, "agent error");
            activity_log::push_activity(logs, agent_name, ActivityEntry::Status {
                activity: "error".to_string(),
                detail: message.clone(),
            });
        }
    }
}

impl AgentLifecycle for AgentManager {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(AgentManager::start_agent(self, agent_name))
    }

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(AgentManager::notify_agent(self, agent_name))
    }

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(AgentManager::stop_agent(self, agent_name))
    }

    fn get_activity_log_data(&self, agent_name: &str, after_seq: Option<u64>) -> ActivityLogResponse {
        activity_log::get_activity_log(&self.activity_logs, agent_name, after_seq)
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        activity_log::all_activity_states(&self.activity_logs)
    }
}

// ── Prompt builder for start/resume ──

fn build_start_prompt(
    config: &AgentConfig,
    driver: &dyn Driver,
    is_codex_driver: bool,
    is_resume: bool,
    unread_summary: &std::collections::HashMap<String, i64>,
) -> String {
    if is_codex_driver || !is_resume {
        return driver.build_system_prompt(config, &config.name);
    }

    let prefix = driver.mcp_tool_prefix();

    if !unread_summary.is_empty() {
        let mut prompt = String::from("You have unread messages from while you were offline:");
        for (ch, count) in unread_summary {
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
    }
}
