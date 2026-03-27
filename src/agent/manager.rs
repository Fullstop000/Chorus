use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::agent::activity_log::{self, ActivityEntry, ActivityLogMap, ActivityLogResponse};
use crate::agent::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::agent::AgentLifecycle;
use crate::store::agents::{AgentConfig, AgentStatus};
use crate::store::messages::ReceivedMessage;
use crate::store::Store;

struct RunningAgent {
    instance_id: u64,
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
    next_instance_id: AtomicU64,
}

fn get_driver(runtime: &str) -> anyhow::Result<Arc<dyn Driver>> {
    match runtime {
        "claude" => Ok(Arc::new(crate::agent::drivers::claude::ClaudeDriver)),
        "codex" => Ok(Arc::new(crate::agent::drivers::codex::CodexDriver)),
        "kimi" => Ok(Arc::new(crate::agent::drivers::kimi::KimiDriver)),
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
            next_instance_id: AtomicU64::new(1),
        }
    }

    /// Start an agent process. Creates the workspace, writes `MEMORY.md`, and
    /// optionally threads through the message that caused the wake-up.
    pub async fn start_agent(
        &self,
        agent_name: &str,
        wake_message: Option<ReceivedMessage>,
    ) -> anyhow::Result<()> {
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
        let resumable_session_id = match driver.id() {
            "codex" => agent.session_id.clone(),
            "kimi" => Some(
                agent
                    .session_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            ),
            _ => None,
        };

        let config = AgentConfig {
            name: agent.name.clone(),
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            session_id: resumable_session_id,
            reasoning_effort: agent.reasoning_effort.clone(),
            env_vars: agent.env_vars.clone(),
            teams: self
                .store
                .list_teams_for_agent(agent_name)
                .unwrap_or_default(),
        };

        let agent_data_dir = self.data_dir.join(agent_name);
        tokio::fs::create_dir_all(&agent_data_dir).await?;

        let memory_md_path = agent_data_dir.join("MEMORY.md");
        if !memory_md_path.exists() {
            let description = config
                .description
                .as_deref()
                .unwrap_or("No role defined yet.");
            tokio::fs::write(
                &memory_md_path,
                format!(
                    "# {}\n\n## Role\n{}\n\n## Key Knowledge\n- No notes yet.\n\n## Active Context\n- First startup.\n",
                    config.display_name, description
                ),
            ).await?;
        }
        tokio::fs::create_dir_all(agent_data_dir.join("notes")).await?;

        let is_resume = config.session_id.is_some();
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        let prompt = build_start_prompt(
            &config,
            driver.as_ref(),
            is_resume,
            &unread_summary,
            wake_message.as_ref(),
        );

        let running_session_id = config.session_id.clone();
        if driver.id() == "kimi" {
            if let Some(ref session_id) = running_session_id {
                self.store
                    .update_agent_session(agent_name, Some(session_id.as_str()))?;
            }
        }

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
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture agent stdout"))?;
        let stderr = child.stderr.take();

        let instance_id = self.next_instance_id.fetch_add(1, Ordering::Relaxed);

        {
            let mut agents = self.agents.lock().await;
            agents.insert(
                agent_name.to_string(),
                RunningAgent {
                    instance_id,
                    process: child,
                    driver: driver.clone(),
                    session_id: running_session_id,
                    is_in_receive_message: false,
                    pending_notification_count: 0,
                },
            );
        }

        self.store
            .update_agent_status(agent_name, AgentStatus::Active)?;
        activity_log::set_activity_state(&self.activity_logs, agent_name, "working", "Starting…");

        self.spawn_output_reader(agent_name.to_string(), stdout, driver, instance_id);
        if let Some(stderr) = stderr {
            self.spawn_stderr_reader(agent_name.to_string(), stderr, instance_id);
        }
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
        self.store
            .update_agent_status(agent_name, AgentStatus::Inactive)?;
        activity_log::set_activity_state(
            &self.activity_logs,
            agent_name,
            "offline",
            "Process stopped",
        );
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
        self.store
            .update_agent_status(agent_name, AgentStatus::Sleeping)?;
        activity_log::set_activity_state(&self.activity_logs, agent_name, "offline", "Sleeping");
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
                let check_tool = format!("{}check_messages", running.driver.mcp_tool_prefix());
                let notification = format!(
                    "[System notification: You have {current_count} new message{plural} waiting. \
                     Call {check_tool} to read {them} when you're ready.]"
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
        instance_id: u64,
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

                if driver.id() == "kimi" {
                    debug!(agent = %name, raw_stdout = %line, "raw agent stdout");
                    activity_log::push_activity(
                        &activity_logs,
                        &name,
                        ActivityEntry::RawOutput { text: line.clone() },
                    );
                }

                for event in driver.parse_line(&line) {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        handle_parsed_event(&agents, &activity_logs, &store, &name, event, &driver)
                            .await;
                    });
                }
            }

            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut agents_map = agents.lock().await;
                let Some(current) = agents_map.get(&name) else {
                    info!(agent = %name, instance_id, "stdout reader ended for stale process");
                    return;
                };
                if current.instance_id != instance_id {
                    info!(agent = %name, stale_instance = instance_id, current_instance = current.instance_id, "stdout reader ended after agent restart");
                    return;
                }

                info!(agent = %name, "stdout reader ended — checking exit status");
                activity_log::set_activity_state(&activity_logs, &name, "offline", "Process stopped");

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

    fn spawn_stderr_reader(
        &self,
        agent_name: String,
        stderr: std::process::ChildStderr,
        instance_id: u64,
    ) {
        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        error!(agent = %agent_name, err = %e, "stderr read error");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                warn!(agent = %agent_name, instance_id, raw_stderr = %line, "agent stderr");
            }
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
            let preview = if text.chars().count() > 120 {
                format!("{preview}…")
            } else {
                preview
            };
            debug!(agent = %agent_name, text = %preview, "thinking");
            activity_log::push_activity(
                logs,
                agent_name,
                ActivityEntry::Thinking { text: text.clone() },
            );
            activity_log::set_activity_state(logs, agent_name, "thinking", "Thinking…");
        }
        ParsedEvent::Text { ref text } => {
            running.is_in_receive_message = false;
            let preview: String = text.chars().take(120).collect();
            let preview = if text.chars().count() > 120 {
                format!("{preview}…")
            } else {
                preview
            };
            info!(agent = %agent_name, text = %preview, "text output");
            activity_log::push_activity(
                logs,
                agent_name,
                ActivityEntry::Text { text: text.clone() },
            );
        }
        ParsedEvent::ToolCall {
            ref name,
            ref input,
        } => {
            let receive_tool = format!("{}receive_message", driver.mcp_tool_prefix());
            let wait_tool = format!("{}wait_for_message", driver.mcp_tool_prefix());
            if *name == receive_tool || *name == wait_tool {
                running.is_in_receive_message = true;
                running.pending_notification_count = 0;
                info!(agent = %agent_name, "waiting for messages");
                let display_name = driver.tool_display_name(name);
                activity_log::push_activity(
                    logs,
                    agent_name,
                    ActivityEntry::ToolStart {
                        tool_name: display_name,
                        tool_input: String::new(),
                    },
                );
                activity_log::set_activity_state(
                    logs,
                    agent_name,
                    "online",
                    "Waiting for messages",
                );
            } else {
                running.is_in_receive_message = false;
                let display_name = driver.tool_display_name(name);
                let tool_input = driver.summarize_tool_input(name, input);
                info!(agent = %agent_name, tool = %name, input = %tool_input, "tool call");
                activity_log::push_activity(
                    logs,
                    agent_name,
                    ActivityEntry::ToolStart {
                        tool_name: display_name.clone(),
                        tool_input,
                    },
                );
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
            activity_log::push_activity(
                logs,
                agent_name,
                ActivityEntry::Status {
                    activity: "error".to_string(),
                    detail: message.clone(),
                },
            );
        }
    }
}

impl AgentLifecycle for AgentManager {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        wake_message: Option<ReceivedMessage>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(AgentManager::start_agent(self, agent_name, wake_message))
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

    fn get_activity_log_data(
        &self,
        agent_name: &str,
        after_seq: Option<u64>,
    ) -> ActivityLogResponse {
        activity_log::get_activity_log(&self.activity_logs, agent_name, after_seq)
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        activity_log::all_activity_states(&self.activity_logs)
    }

    fn push_activity_entry(&self, agent_name: &str, entry: ActivityEntry) {
        activity_log::push_activity(&self.activity_logs, agent_name, entry);
    }
}

// ── Prompt builder for start/resume ──

fn build_start_prompt(
    config: &AgentConfig,
    driver: &dyn Driver,
    is_resume: bool,
    unread_summary: &std::collections::HashMap<String, i64>,
    wake_message: Option<&ReceivedMessage>,
) -> String {
    if let Some(wake_message) = wake_message {
        let wake_prompt = build_wake_message_prompt(driver, wake_message, unread_summary);
        if is_resume {
            return wake_prompt;
        }
        return format!(
            "{}\n\n{}",
            driver.build_system_prompt(config, &config.name),
            wake_prompt
        );
    }

    if !is_resume {
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
             then call the wait_for_message tool to return to the idle loop.",
        );
        if driver.supports_stdin_notification() {
            prompt.push_str(&format!(
                "\n\nNote: While you are busy, you may receive \
                 [System notification: ...] messages. \
                 Finish your current step, then call {prefix}check_messages().",
            ));
        }
        prompt
    } else {
        let mut prompt = format!(
            "No new messages while you were away. \
             Call {prefix}wait_for_message() to listen for new messages."
        );
        if driver.supports_stdin_notification() {
            prompt.push_str(&format!(
                "\n\nNote: While you are busy, you may receive \
                 [System notification: ...] messages about new messages. \
                 Finish your current step, then call {prefix}check_messages().",
            ));
        }
        prompt
    }
}

/// Format the specific unread message that caused an agent restart so the
/// resumed runtime knows why it woke up without treating the preview as the
/// authoritative source of truth.
fn build_wake_message_prompt(
    driver: &dyn Driver,
    wake_message: &ReceivedMessage,
    unread_summary: &std::collections::HashMap<String, i64>,
) -> String {
    let prefix = driver.mcp_tool_prefix();
    let target = format_message_target(wake_message);
    let attachment_count = wake_message.attachments.as_ref().map_or(0, Vec::len);
    let content_preview = truncate_prompt_text(&wake_message.content, 2_000);

    let mut prompt = format!(
        "You were just woken by a new unread message.\n\
         Treat this preview as wake-up context only. The message is still unread.\n\
         Call {prefix}check_messages() now to load unread messages before you respond.\n\n\
         Triggering message:\n\
         - From: {}\n\
         - Target: {target}\n\
         - Timestamp: {}\n\
         - Attachments: {attachment_count}\n\
         - Content:\n{}",
        wake_message.sender_name, wake_message.timestamp, content_preview
    );

    if !unread_summary.is_empty() {
        prompt.push_str("\n\nUnread summary:");
        for (channel_name, count) in unread_summary {
            prompt.push_str(&format!("\n- {channel_name}: {count} unread"));
        }
    }

    prompt.push_str(&format!(
        "\n\nAfter you finish the triggered work, return to {prefix}wait_for_message()."
    ));

    prompt
}

/// Convert the stored message shape into the human-facing target label used in
/// prompts. For threads we preserve the exact reply target so restarted
/// runtimes can respond in-place without reconstructing the short id.
fn format_message_target(message: &ReceivedMessage) -> String {
    match message.channel_type.as_str() {
        "dm" => format!("dm:@{}", message.channel_name),
        "thread" => {
            let parent_name = message
                .parent_channel_name
                .as_deref()
                .unwrap_or(&message.channel_name);
            let short_id = message
                .channel_name
                .strip_prefix("thread-")
                .unwrap_or(&message.channel_name);
            match message.parent_channel_type.as_deref() {
                Some("dm") => format!("dm:@{parent_name}:{short_id}"),
                _ => format!("#{parent_name}:{short_id}"),
            }
        }
        _ => format!("#{}", message.channel_name),
    }
}

/// Keep wake-up previews bounded so a single long message does not dominate the
/// restart prompt.
fn truncate_prompt_text(text: &str, max_chars: usize) -> String {
    let truncated: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        format!("{truncated}\n[truncated]")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    struct FakeDriver;

    impl Driver for FakeDriver {
        fn id(&self) -> &str {
            "codex"
        }

        fn supports_stdin_notification(&self) -> bool {
            false
        }

        fn mcp_tool_prefix(&self) -> &str {
            "mcp_chat_"
        }

        fn spawn(&self, _ctx: &SpawnContext) -> anyhow::Result<Child> {
            unreachable!("spawn is not used by prompt unit tests")
        }

        fn parse_line(&self, _line: &str) -> Vec<ParsedEvent> {
            vec![]
        }

        fn encode_stdin_message(&self, _text: &str, _session_id: &str) -> Option<String> {
            None
        }

        fn build_system_prompt(&self, _config: &AgentConfig, _agent_id: &str) -> String {
            "BASE PROMPT".to_string()
        }

        fn tool_display_name(&self, name: &str) -> String {
            name.to_string()
        }

        fn summarize_tool_input(&self, _name: &str, _input: &serde_json::Value) -> String {
            String::new()
        }
    }

    fn sample_config(session_id: Option<&str>) -> AgentConfig {
        AgentConfig {
            name: "bot1".to_string(),
            display_name: "Bot 1".to_string(),
            description: Some("Replies in Chorus".to_string()),
            runtime: "codex".to_string(),
            model: "gpt-5.4-mini".to_string(),
            session_id: session_id.map(str::to_string),
            reasoning_effort: None,
            env_vars: Vec::new(),
            teams: vec![],
        }
    }

    fn sample_wake_message() -> ReceivedMessage {
        serde_json::from_value(json!({
            "message_id": "msg-1",
            "channel_name": "general",
            "channel_type": "channel",
            "sender_name": "alice",
            "sender_type": "human",
            "content": "Please investigate the Codex restart path.",
            "timestamp": "2026-03-22T12:00:00Z"
        }))
        .expect("wake message fixture should deserialize")
    }

    fn sample_thread_wake_message() -> ReceivedMessage {
        serde_json::from_value(json!({
            "message_id": "reply-1",
            "channel_name": "thread-a1b2c3d4",
            "channel_type": "thread",
            "parent_channel_name": "eng-team",
            "parent_channel_type": "channel",
            "sender_name": "alice",
            "sender_type": "human",
            "content": "Please reply in the team thread.",
            "timestamp": "2026-03-22T12:00:00Z"
        }))
        .expect("thread wake message fixture should deserialize")
    }

    #[test]
    fn wake_prompt_for_resumed_agent_mentions_check_and_wait_tools() {
        let config = sample_config(Some("thread-123"));
        let driver = FakeDriver;
        let prompt = build_start_prompt(
            &config,
            &driver,
            true,
            &std::collections::HashMap::new(),
            Some(&sample_wake_message()),
        );

        assert!(prompt.contains("woken by a new unread message"));
        assert!(prompt.contains("mcp_chat_check_messages() now"));
        assert!(prompt.contains("mcp_chat_wait_for_message()"));
        assert!(prompt.contains("Please investigate the Codex restart path."));
        assert!(
            !prompt.contains("BASE PROMPT"),
            "resume prompt should send only the wake-specific delta"
        );
    }

    #[test]
    fn wake_prompt_for_fresh_agent_keeps_base_prompt() {
        let config = sample_config(None);
        let driver = FakeDriver;
        let prompt = build_start_prompt(
            &config,
            &driver,
            false,
            &std::collections::HashMap::new(),
            Some(&sample_wake_message()),
        );

        assert!(prompt.contains("BASE PROMPT"));
        assert!(prompt.contains("Treat this preview as wake-up context only."));
        assert!(prompt.contains("Target: #general"));
    }

    #[test]
    fn wake_prompt_for_thread_message_uses_exact_reply_target() {
        let config = sample_config(Some("thread-123"));
        let driver = FakeDriver;
        let prompt = build_start_prompt(
            &config,
            &driver,
            true,
            &std::collections::HashMap::new(),
            Some(&sample_thread_wake_message()),
        );

        assert!(prompt.contains("Target: #eng-team:a1b2c3d4"));
        assert!(
            !prompt.contains("Target: #eng-team thread"),
            "wake prompts must preserve the exact thread target so resumed agents can reply in place"
        );
    }

    #[tokio::test]
    async fn stale_output_reader_does_not_remove_restarted_agent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("chorus.db");
        let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
        store
            .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
            .unwrap();

        let manager = AgentManager::new(
            store,
            dir.path().join("agents"),
            "chorus".to_string(),
            "http://127.0.0.1:3001".to_string(),
        );
        let driver: Arc<dyn Driver> = Arc::new(FakeDriver);

        let mut first = Command::new("sh")
            .arg("-c")
            .arg("sleep 0.1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let first_stdout = first.stdout.take().unwrap();
        {
            let mut agents = manager.agents.lock().await;
            agents.insert(
                "bot1".to_string(),
                RunningAgent {
                    instance_id: 1,
                    process: first,
                    driver: driver.clone(),
                    session_id: None,
                    is_in_receive_message: false,
                    pending_notification_count: 0,
                },
            );
        }
        manager.spawn_output_reader("bot1".to_string(), first_stdout, driver.clone(), 1);

        {
            let mut agents = manager.agents.lock().await;
            agents.remove("bot1").unwrap();
        }

        let second = Command::new("sh")
            .arg("-c")
            .arg("sleep 1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut agents = manager.agents.lock().await;
            agents.insert(
                "bot1".to_string(),
                RunningAgent {
                    instance_id: 2,
                    process: second,
                    driver,
                    session_id: None,
                    is_in_receive_message: false,
                    pending_notification_count: 0,
                },
            );
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let contains_restarted_agent = manager.agents.lock().await.contains_key("bot1");
        if contains_restarted_agent {
            if let Some(mut running) = manager.agents.lock().await.remove("bot1") {
                let _ = running.process.kill();
                let _ = running.process.wait();
            }
        }

        assert!(
            contains_restarted_agent,
            "stale stdout reader removed the restarted agent from the running map"
        );
    }
}
