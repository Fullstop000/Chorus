use tracing::{debug, error, info, warn};
use crate::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::models::*;
use crate::server::AgentLifecycle;
use crate::store::Store;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const ACTIVITY_LOG_MAX: usize = 500;

struct RunningAgent {
    process: Child,
    driver: Arc<dyn Driver>,
    session_id: Option<String>,
    is_in_receive_message: bool,
    pending_notification_count: u32,
}

/// Per-agent in-memory activity log (ring buffer, up to ACTIVITY_LOG_MAX entries).
#[derive(Default)]
struct AgentActivityLog {
    entries: std::collections::VecDeque<ActivityLogEntry>,
    next_seq: u64,
    /// Current activity state: online | thinking | working | offline
    activity: String,
    detail: String,
}

impl AgentActivityLog {
    fn push(&mut self, entry: ActivityEntry) {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.entries.push_back(ActivityLogEntry {
            seq: self.next_seq,
            timestamp_ms,
            entry,
        });
        self.next_seq += 1;
        if self.entries.len() > ACTIVITY_LOG_MAX {
            self.entries.pop_front();
        }
    }

    fn since(&self, after_seq: u64) -> Vec<ActivityLogEntry> {
        self.entries
            .iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect()
    }

    fn all(&self) -> Vec<ActivityLogEntry> {
        self.entries.iter().cloned().collect()
    }
}

pub struct AgentManager {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    /// Activity logs keyed by agent name
    pub activity_logs: Arc<std::sync::Mutex<HashMap<String, AgentActivityLog>>>,
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

    /// Get activity log entries for an agent (optionally after a seq number).
    pub fn get_activity_log(
        &self,
        agent_name: &str,
        after_seq: Option<u64>,
    ) -> ActivityLogResponse {
        let logs = self.activity_logs.lock().unwrap();
        let log = logs.get(agent_name);
        let (entries, activity, detail) = match log {
            Some(l) => {
                let entries = match after_seq {
                    Some(seq) => l.since(seq),
                    None => l.all(),
                };
                (entries, l.activity.clone(), l.detail.clone())
            }
            None => (vec![], "offline".to_string(), String::new()),
        };
        ActivityLogResponse { entries, agent_activity: activity, agent_detail: detail }
    }

    fn push_activity(
        activity_logs: &Arc<std::sync::Mutex<HashMap<String, AgentActivityLog>>>,
        agent_name: &str,
        entry: ActivityEntry,
    ) {
        let mut logs = activity_logs.lock().unwrap();
        let log = logs.entry(agent_name.to_string()).or_default();
        log.push(entry);
    }

    fn set_activity_state(
        activity_logs: &Arc<std::sync::Mutex<HashMap<String, AgentActivityLog>>>,
        agent_name: &str,
        activity: &str,
        detail: &str,
    ) {
        let mut logs = activity_logs.lock().unwrap();
        let log = logs.entry(agent_name.to_string()).or_default();
        log.activity = activity.to_string();
        log.detail = detail.to_string();
        let entry = ActivityEntry::Status {
            activity: activity.to_string(),
            detail: detail.to_string(),
        };
        log.push(entry);
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
        let is_codex_driver = driver.id() == "codex";

        // Build AgentConfig
        let config = AgentConfig {
            name: agent.name.clone(),
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            session_id: if is_codex_driver {
                None
            } else {
                agent.session_id.clone()
            },
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
        let is_resume = !is_codex_driver && agent.session_id.is_some();

        // Get unread summary
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        // Build the initial prompt
        let prompt = if is_codex_driver || !is_resume {
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
            agent_id: agent.name.clone(),
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

        // Update status to active + emit activity log entry
        self.store
            .update_agent_status(agent_name, AgentStatus::Active)?;
        Self::set_activity_state(
            &self.activity_logs,
            agent_name,
            "working",
            "Starting…",
        );

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

        info!(agent = %agent_name, "hibernating (sleeping)");

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

                let events = driver.parse_line(&line);
                for event in events {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        Self::handle_parsed_event(
                            &agents,
                            &activity_logs,
                            &store,
                            &name,
                            event,
                            &driver,
                        )
                        .await;
                    });
                }
            }

            // Process exited — stdout closed
            info!(agent = %name, "stdout reader ended — checking exit status");
            Self::set_activity_state(&activity_logs, &name, "offline", "Process stopped");

            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut agents_map = agents.lock().await;
                if let Some(mut running) = agents_map.remove(&name) {
                    match running.process.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            if code == 0 {
                                info!(agent = %name, code, "process exited cleanly — sleeping");
                                let _ =
                                    store.update_agent_status(&name, AgentStatus::Sleeping);
                            } else {
                                warn!(agent = %name, code, "process crashed — marking inactive");
                                let _ =
                                    store.update_agent_status(&name, AgentStatus::Inactive);
                            }
                        }
                        Err(e) => {
                            error!(agent = %name, err = %e, "failed to get exit status");
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
        activity_logs: &Arc<std::sync::Mutex<HashMap<String, AgentActivityLog>>>,
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
                Self::set_activity_state(activity_logs, agent_name, "online", "Ready");
            }
            ParsedEvent::Thinking { ref text } => {
                running.is_in_receive_message = false;
                let preview: String = text.chars().take(120).collect();
                let preview = if text.chars().count() > 120 { format!("{preview}…") } else { preview };
                debug!(agent = %agent_name, text = %preview, "thinking");
                Self::push_activity(
                    activity_logs,
                    agent_name,
                    ActivityEntry::Thinking { text: text.clone() },
                );
                Self::set_activity_state(activity_logs, agent_name, "thinking", "Thinking…");
            }
            ParsedEvent::Text { ref text } => {
                running.is_in_receive_message = false;
                let preview: String = text.chars().take(120).collect();
                let preview = if text.chars().count() > 120 { format!("{preview}…") } else { preview };
                info!(agent = %agent_name, text = %preview, "text output");
                Self::push_activity(
                    activity_logs,
                    agent_name,
                    ActivityEntry::Text { text: text.clone() },
                );
            }
            ParsedEvent::ToolCall { ref name, ref input } => {
                let receive_tool = format!("{}receive_message", driver.mcp_tool_prefix());
                if *name == receive_tool {
                    running.is_in_receive_message = true;
                    running.pending_notification_count = 0;
                    info!(agent = %agent_name, "waiting for messages");
                    Self::push_activity(
                        activity_logs,
                        agent_name,
                        ActivityEntry::ToolStart {
                            tool_name: name.clone(),
                            tool_input: String::new(),
                        },
                    );
                    Self::set_activity_state(activity_logs, agent_name, "online", "Waiting for messages");
                } else {
                    running.is_in_receive_message = false;
                    let display_name = driver.tool_display_name(name);
                    let tool_input = driver.summarize_tool_input(name, input);
                    info!(agent = %agent_name, tool = %name, input = %tool_input, "tool call");
                    Self::push_activity(
                        activity_logs,
                        agent_name,
                        ActivityEntry::ToolStart {
                            tool_name: display_name.clone(),
                            tool_input,
                        },
                    );
                    Self::set_activity_state(activity_logs, agent_name, "working", &display_name);
                }
            }
            ParsedEvent::TurnEnd { session_id } => {
                info!(agent = %agent_name, "turn ended");
                running.is_in_receive_message = false;
                if let Some(ref sid) = session_id {
                    running.session_id = Some(sid.clone());
                    let _ = store.update_agent_session(agent_name, Some(sid));
                }
                Self::set_activity_state(activity_logs, agent_name, "online", "Idle");
            }
            ParsedEvent::Error { ref message } => {
                error!(agent = %agent_name, message = %message, "agent error");
                Self::push_activity(
                    activity_logs,
                    agent_name,
                    ActivityEntry::Status {
                        activity: "error".to_string(),
                        detail: message.clone(),
                    },
                );
            }
        }
    }
}

impl AgentLifecycle for AgentManager {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move { AgentManager::start_agent(self, agent_name).await })
    }

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move { AgentManager::notify_agent(self, agent_name).await })
    }

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move { AgentManager::stop_agent(self, agent_name).await })
    }

    fn get_activity_log_data(&self, agent_name: &str, after_seq: Option<u64>) -> ActivityLogResponse {
        AgentManager::get_activity_log(self, agent_name, after_seq)
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        let logs = self.activity_logs.lock().unwrap();
        logs.iter()
            .map(|(name, log)| (name.clone(), log.activity.clone(), log.detail.clone()))
            .collect()
    }
}
