use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{error, info, trace, warn};

use crate::agent::activity_log::{self, ActivityEntry, ActivityLogMap, ActivityLogResponse};
use crate::agent::config::AgentConfig;
use crate::agent::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::agent::AgentLifecycle;
use crate::agent::AgentRuntime;
use crate::store::agents::AgentStatus;
use crate::store::messages::ReceivedMessage;
use crate::store::Store;

struct RunningAgent {
    instance_id: u64,
    process: Child,
    driver: Arc<dyn Driver>,
    session_id: Option<String>,
    pending_notification_count: u32,
    last_tool_raw_name: Option<String>,
    /// Accumulated thinking chunks, flushed to log/activity on the next non-Thinking event.
    pending_thinking: String,
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
    match AgentRuntime::parse(runtime) {
        Some(rt) => Ok(crate::agent::drivers::driver_for_runtime(rt)),
        None => anyhow::bail!("Unknown runtime: {runtime}"),
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
        // Raw Kimi driver requires a pre-generated session id (it uses
        // stdin notifications, so supports_stdin_notification()=true).
        // ACP drivers handle sessions internally via session/new|load.
        let resumable_session_id =
            if driver.runtime() == AgentRuntime::Kimi && driver.supports_stdin_notification() {
                Some(
                    agent
                        .session_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                )
            } else {
                agent.session_id.clone()
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

        let is_resume = agent.session_id.is_some();
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        let prompt = build_start_prompt(
            &config,
            driver.as_ref(),
            is_resume,
            &unread_summary,
            wake_message.as_ref(),
        );

        let running_session_id = config.session_id.clone();
        // Pre-write session id for raw Kimi driver (it reads from store on spawn).
        if driver.runtime() == AgentRuntime::Kimi && driver.supports_stdin_notification() {
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
                    pending_notification_count: 0,
                    last_tool_raw_name: None,
                    pending_thinking: String::new(),
                },
            );
        }

        self.store
            .update_agent_status(agent_name, AgentStatus::Active)?;
        activity_log::set_activity_state(
            &self.activity_logs,
            agent_name,
            "working",
            "Starting\u{2026}",
        );
        activity_log::push_activity(
            &self.activity_logs,
            agent_name,
            ActivityEntry::Start { is_resume },
        );

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
        // Clear persisted session so next start uses session/new.
        let _ = self.store.update_agent_session(agent_name, None);
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
        // Clear persisted session so next start uses session/new.
        let _ = self.store.update_agent_session(agent_name, None);
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

        if !running.driver.supports_stdin_notification() {
            return Ok(());
        }

        if running.session_id.is_none() {
            // Agent is still initializing. Count the notification so it is
            // delivered via stdin as soon as SessionInit is received.
            running.pending_notification_count += 1;
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
                if current_count == 0 || current_count != count {
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
                        error!(agent = %name, err = %e, backtrace = %project_backtrace(), "stdout read error");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }

                // ACP runtimes log raw stdout at trace level so `RUST_LOG=chorus=trace`
                // gives a full wire dump without needing temporary code changes.
                // Skip per-chunk streaming lines to avoid log floods.
                if (driver.runtime() == AgentRuntime::Kimi
                    || driver.runtime() == AgentRuntime::Opencode)
                    && !line.contains("agent_message_chunk")
                    && !line.contains("agentMessageChunk")
                    && !line.contains("agent_thought_chunk")
                    && !line.contains("agentThoughtChunk")
                    && !line.contains("tool_call_update")
                    && !line.contains("toolCallUpdate")
                {
                    trace!(agent = %name, raw_stdout = %line, "raw agent stdout");
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
                            // Clear persisted session so next start uses session/new.
                            let _ = store.update_agent_session(&name, None);
                        }
                        Err(e) => {
                            error!(agent = %name, err = %e, backtrace = %project_backtrace(), "failed to get exit status");
                            let _ = store.update_agent_status(&name, AgentStatus::Inactive);
                            let _ = store.update_agent_session(&name, None);
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
                        error!(agent = %agent_name, err = %e, backtrace = %project_backtrace(), "stderr read error");
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

    // Flush accumulated thinking when a non-Thinking event arrives.
    let is_thinking_event = matches!(event, ParsedEvent::Thinking { .. });
    if !is_thinking_event && !running.pending_thinking.is_empty() {
        let full_thought = std::mem::take(&mut running.pending_thinking);
        let preview: String = full_thought.chars().take(200).collect();
        let preview = if full_thought.chars().count() > 200 {
            format!("{preview}…")
        } else {
            preview
        };
        trace!(agent = %agent_name, thought = %preview, "thinking block complete");
        activity_log::push_activity(
            logs,
            agent_name,
            ActivityEntry::Thinking { text: full_thought },
        );
    }

    match event {
        ParsedEvent::SessionInit { session_id } => {
            info!(agent = %agent_name, session = %session_id, "session started");
            running.session_id = Some(session_id.clone());
            let _ = store.update_agent_session(agent_name, Some(&session_id));
            activity_log::set_activity_state(logs, agent_name, "online", "Ready");

            // Flush notifications that arrived before the session was ready.
            let pending = running.pending_notification_count;
            if pending > 0 && running.driver.supports_stdin_notification() {
                running.pending_notification_count = 0;
                let plural = if pending > 1 { "s" } else { "" };
                let them = if pending > 1 { "them" } else { "it" };
                let check_tool = format!("{}check_messages", running.driver.mcp_tool_prefix());
                let notification = format!(
                    "[System notification: You have {pending} new message{plural} waiting. \
                     Call {check_tool} to read {them} when you're ready.]"
                );
                if let Some(encoded) = running
                    .driver
                    .encode_stdin_message(&notification, &session_id)
                {
                    if let Some(stdin) = running.process.stdin.as_mut() {
                        let _ = writeln!(stdin, "{encoded}");
                    }
                }
            }
        }
        ParsedEvent::Thinking { ref text } => {
            running.pending_thinking.push_str(text);
            activity_log::set_activity_state(logs, agent_name, "thinking", "Thinking\u{2026}");
        }
        ParsedEvent::Text { ref text } => {
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
            let display_name = driver.tool_display_name(name);
            let tool_input = driver.summarize_tool_input(name, input);
            info!(agent = %agent_name, tool = %name, input = %tool_input, "tool call");
            running.last_tool_raw_name = Some(name.clone());
            activity_log::push_activity(
                logs,
                agent_name,
                ActivityEntry::ToolCall {
                    tool_name: name.clone(),
                    tool_input,
                },
            );
            activity_log::set_activity_state(logs, agent_name, "working", &display_name);
        }
        ParsedEvent::ToolResult { ref content } => {
            let tool_name = running.last_tool_raw_name.clone().unwrap_or_default();
            activity_log::upsert_tool_result_activity(logs, agent_name, tool_name, content.clone());
        }
        ParsedEvent::TurnEnd { session_id } => {
            info!(agent = %agent_name, "turn ended");
            if let Some(ref sid) = session_id {
                running.session_id = Some(sid.clone());
                let _ = store.update_agent_session(agent_name, Some(sid));
            }
            activity_log::set_activity_state(logs, agent_name, "online", "Idle");
        }
        ParsedEvent::Error { ref message } => {
            error!(agent = %agent_name, message = %message, backtrace = %project_backtrace(), "agent error");
            activity_log::set_activity_state(logs, agent_name, "error", message);
        }
        ParsedEvent::WriteStdin { ref data } => {
            if let Some(stdin) = running.process.stdin.as_mut() {
                let _ = write!(stdin, "{data}");
            }
        }
        ParsedEvent::PermissionRequested { tool_name: _ } => {
            // Permission approval is handled inline — the response has already been
            // written to stdin by WriteStdin. No retry needed.
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
             then stop. New messages will be delivered to you automatically.",
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
        let mut prompt =
            "No new messages while you were away. Nothing to do right now — just stop.".to_string();
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

/// Capture a backtrace filtered to frames within this crate.
/// Falls back to the full backtrace if no project frames are found
/// (e.g. in release builds with debug info stripped).
fn project_backtrace() -> String {
    let full = std::backtrace::Backtrace::capture().to_string();
    let project_frames: Vec<&str> = full
        .lines()
        .filter(|line| line.contains("chorus"))
        .take(15)
        .collect();
    if project_frames.is_empty() {
        full
    } else {
        project_frames.join("\n")
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
        fn runtime(&self) -> AgentRuntime {
            AgentRuntime::Codex
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

        fn detect_runtime_status(
            &self,
        ) -> anyhow::Result<crate::agent::runtime_status::RuntimeStatus> {
            Ok(crate::agent::runtime_status::RuntimeStatus {
                runtime: self.id().to_string(),
                installed: true,
                auth_status: Some(crate::agent::runtime_status::RuntimeAuthStatus::Authed),
            })
        }

        fn list_models(&self) -> anyhow::Result<Vec<String>> {
            Ok(vec!["gpt-5.4-mini".to_string()])
        }
    }

    // ── Chunk accumulation tests ──

    /// Helper: create a minimal RunningAgent backed by a real (sleeping) child process.
    fn make_running_agent(driver: Arc<dyn Driver>) -> RunningAgent {
        let mut proc = Command::new("sh")
            .arg("-c")
            .arg("sleep 1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        proc.stdout.take(); // detach so no reader is needed
        RunningAgent {
            instance_id: 99,
            process: proc,
            driver,
            session_id: Some("s1".to_string()),
            pending_notification_count: 0,
            last_tool_raw_name: None,
            pending_thinking: String::new(),
        }
    }

    #[tokio::test]
    async fn thinking_chunks_accumulate_and_flush_as_single_activity_entry() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        let agents: Arc<Mutex<HashMap<String, RunningAgent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let logs: Arc<ActivityLogMap> = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let driver: Arc<dyn Driver> = Arc::new(FakeDriver);

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        // Feed three thinking chunks — should not appear in the log yet.
        for chunk in &["Let me ", "think ", "carefully."] {
            handle_parsed_event(
                &agents,
                &logs,
                &store,
                "bot1",
                ParsedEvent::Thinking {
                    text: chunk.to_string(),
                },
                &driver,
            )
            .await;
        }
        let pre_flush = activity_log::get_activity_log(&logs, "bot1", None).entries;
        let pre_thinking_count = pre_flush
            .iter()
            .filter(|e| matches!(e.entry, ActivityEntry::Thinking { .. }))
            .count();
        assert_eq!(
            pre_thinking_count, 0,
            "thinking chunks must not be pushed until flushed"
        );

        // A non-Thinking event flushes the buffer.
        handle_parsed_event(
            &agents,
            &logs,
            &store,
            "bot1",
            ParsedEvent::TurnEnd { session_id: None },
            &driver,
        )
        .await;

        let entries = activity_log::get_activity_log(&logs, "bot1", None).entries;
        let thinking_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry, ActivityEntry::Thinking { .. }))
            .collect();
        assert_eq!(
            thinking_entries.len(),
            1,
            "all thinking chunks must flush as exactly one Thinking entry"
        );
        if let ActivityEntry::Thinking { ref text } = thinking_entries[0].entry {
            assert_eq!(text, "Let me think carefully.");
        } else {
            panic!("expected Thinking entry");
        }

        {
            let mut map = agents.lock().await;
            if let Some(mut r) = map.remove("bot1") {
                let _ = r.process.kill();
            }
        }
    }

    #[tokio::test]
    async fn message_chunks_each_create_separate_text_entry() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        let agents: Arc<Mutex<HashMap<String, RunningAgent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let logs: Arc<ActivityLogMap> = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let driver: Arc<dyn Driver> = Arc::new(FakeDriver);

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        for chunk in &["Hello ", "world", "!"] {
            handle_parsed_event(
                &agents,
                &logs,
                &store,
                "bot1",
                ParsedEvent::Text {
                    text: chunk.to_string(),
                },
                &driver,
            )
            .await;
        }

        let entries = activity_log::get_activity_log(&logs, "bot1", None).entries;
        let text_entries: Vec<_> = entries
            .iter()
            .filter_map(|e| {
                if let ActivityEntry::Text { ref text } = e.entry {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            text_entries,
            vec!["Hello ", "world", "!"],
            "each message chunk produces its own Text activity entry"
        );

        {
            let mut map = agents.lock().await;
            if let Some(mut r) = map.remove("bot1") {
                let _ = r.process.kill();
            }
        }
    }

    #[tokio::test]
    async fn interleaved_thinking_chunks_flush_before_tool_call() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        let agents: Arc<Mutex<HashMap<String, RunningAgent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let logs: Arc<ActivityLogMap> = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let driver: Arc<dyn Driver> = Arc::new(FakeDriver);

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        // Feed two thinking chunks then a ToolCall.
        for chunk in &["step one ", "step two"] {
            handle_parsed_event(
                &agents,
                &logs,
                &store,
                "bot1",
                ParsedEvent::Thinking {
                    text: chunk.to_string(),
                },
                &driver,
            )
            .await;
        }
        handle_parsed_event(
            &agents,
            &logs,
            &store,
            "bot1",
            ParsedEvent::ToolCall {
                name: "send_message".to_string(),
                input: serde_json::json!({}),
            },
            &driver,
        )
        .await;

        let entries = activity_log::get_activity_log(&logs, "bot1", None).entries;
        let thinking_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry, ActivityEntry::Thinking { .. }))
            .collect();
        assert_eq!(thinking_entries.len(), 1);
        if let ActivityEntry::Thinking { ref text } = thinking_entries[0].entry {
            assert_eq!(text, "step one step two");
        }

        {
            let mut map = agents.lock().await;
            if let Some(mut r) = map.remove("bot1") {
                let _ = r.process.kill();
            }
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
    fn wake_prompt_for_resumed_agent_mentions_check_tool() {
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
        assert!(
            !prompt.contains("wait_for_message"),
            "push-idle: agents no longer use wait_for_message"
        );
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
                    pending_notification_count: 0,
                    last_tool_raw_name: None,
                    pending_thinking: String::new(),
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
                    pending_notification_count: 0,
                    last_tool_raw_name: None,
                    pending_thinking: String::new(),
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


