use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use crate::agent::activity_log::{self, ActivityEntry, ActivityLogMap, ActivityLogResponse};
use crate::agent::drivers::v2::claude::ClaudeDriver;
use crate::agent::drivers::v2::codex::CodexDriver;
use crate::agent::drivers::v2::kimi::KimiDriver;
use crate::agent::drivers::v2::opencode::OpencodeDriver;
use crate::agent::drivers::v2::{
    AgentEventItem, AgentHandle, AgentSpec, AgentState, DriverEvent, PromptReq, RuntimeDriver,
    StartOpts,
};
use crate::agent::trace::{self, AgentTraceStore, TraceEvent, TraceEventKind};
use crate::agent::AgentLifecycle;
use crate::agent::AgentRuntime;
use crate::store::agents::AgentStatus;
use crate::store::messages::ReceivedMessage;
use crate::store::Store;

/// V2-managed agent backed by a [`RuntimeDriver`] + [`AgentHandle`].
struct V2Agent {
    handle: Box<dyn AgentHandle>,
    _event_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Debounce counter for stdin-style notification batching.
    pending_notification_count: u32,
}

pub struct AgentManager {
    /// Driver registry — maps runtime to native v2 driver.
    driver_registry: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>,
    /// Active agents keyed by agent name.
    agents: Arc<Mutex<HashMap<String, V2Agent>>>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
    store: Arc<Store>,
    data_dir: PathBuf,
    bridge_binary: String,
    server_url: String,
}

pub fn build_driver_registry() -> HashMap<AgentRuntime, Arc<dyn RuntimeDriver>> {
    let mut registry: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>> = HashMap::new();
    registry.insert(AgentRuntime::Claude, Arc::new(ClaudeDriver));
    registry.insert(AgentRuntime::Codex, Arc::new(CodexDriver));
    registry.insert(AgentRuntime::Kimi, Arc::new(KimiDriver));
    registry.insert(AgentRuntime::Opencode, Arc::new(OpencodeDriver));
    registry
}

impl AgentManager {
    pub fn new(
        store: Arc<Store>,
        data_dir: PathBuf,
        bridge_binary: String,
        server_url: String,
    ) -> Self {
        Self {
            driver_registry: build_driver_registry(),
            agents: Arc::new(Mutex::new(HashMap::new())),
            activity_logs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            trace_store: Arc::new(AgentTraceStore::new()),
            store,
            data_dir,
            bridge_binary,
            server_url,
        }
    }

    /// Start an agent process. Creates the workspace, writes `MEMORY.md`, and
    /// optionally threads through the message that caused the wake-up.
    pub async fn start_agent(
        &self,
        agent_name: &str,
        wake_message: Option<ReceivedMessage>,
    ) -> anyhow::Result<()> {
        // Already running?
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

        let rt = AgentRuntime::parse(&agent.runtime)
            .ok_or_else(|| anyhow::anyhow!("Unknown runtime: {}", agent.runtime))?;

        let agent_data_dir = self.data_dir.join(agent_name);
        tokio::fs::create_dir_all(&agent_data_dir).await?;

        let memory_md_path = agent_data_dir.join("MEMORY.md");
        if !memory_md_path.exists() {
            let description = agent
                .description
                .as_deref()
                .unwrap_or("No role defined yet.");
            tokio::fs::write(
                &memory_md_path,
                format!(
                    "# {}\n\n## Role\n{}\n\n## Key Knowledge\n- No notes yet.\n\n## Active Context\n- First startup.\n",
                    agent.display_name, description
                ),
            ).await?;
        }
        tokio::fs::create_dir_all(agent_data_dir.join("notes")).await?;

        let v2_driver = self
            .driver_registry
            .get(&rt)
            .ok_or_else(|| anyhow::anyhow!("no driver for runtime {:?}", rt))?
            .clone();

        // Auto-discover the shared bridge — when `chorus serve --shared-bridge`
        // is running, this populates bridge_endpoint so agents connect via HTTP
        // MCP instead of spawning per-agent stdio processes. When no bridge is
        // running, this is None and the legacy stdio path works unchanged.
        let bridge_endpoint = crate::bridge::discovery::read_bridge_info()
            .map(|info| format!("http://127.0.0.1:{}", info.port));

        if let Some(ref endpoint) = bridge_endpoint {
            info!(agent = %agent_name, %endpoint, "starting agent via shared bridge");
        } else {
            debug!(agent = %agent_name, "starting agent via per-agent stdio bridge");
        }

        let spec = AgentSpec {
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            system_prompt: agent.system_prompt.clone(),
            model: agent.model.clone(),
            reasoning_effort: agent.reasoning_effort.clone(),
            env_vars: agent.env_vars.clone(),
            working_directory: agent_data_dir.clone(),
            bridge_binary: self.bridge_binary.clone(),
            server_url: self.server_url.clone(),
            bridge_endpoint,
        };

        let attach_result = v2_driver.attach(agent_name.to_string(), spec).await?;
        let mut handle = attach_result.handle;
        let events = attach_result.events;

        // Subscribe BEFORE start so we don't miss early events.
        let event_rx = events.subscribe();

        let is_resume = agent.session_id.is_some();
        let unread_summary = self.store.get_unread_summary(agent_name)?;

        let init_prompt_text = build_v2_start_prompt(
            &agent.display_name,
            is_resume,
            &unread_summary,
            wake_message.as_ref(),
        );

        let start_opts = StartOpts {
            resume_session_id: agent.session_id.clone(),
        };

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

        handle
            .start(
                start_opts,
                Some(PromptReq {
                    text: init_prompt_text,
                    attachments: vec![],
                }),
            )
            .await?;

        let forwarder = spawn_v2_event_forwarder(
            agent_name.to_string(),
            event_rx,
            self.activity_logs.clone(),
            self.trace_store.clone(),
            self.store.trace_sender(),
            self.store.clone(),
        );

        {
            let mut agents = self.agents.lock().await;
            agents.insert(
                agent_name.to_string(),
                V2Agent {
                    handle,
                    _event_tasks: vec![forwarder],
                    pending_notification_count: 0,
                },
            );
        }

        info!(agent = %agent_name, runtime = %rt.as_str(), "agent started");
        Ok(())
    }

    /// Stop an agent process and mark it inactive.
    pub async fn stop_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut agents = self.agents.lock().await;
        if let Some(mut agent) = agents.remove(agent_name) {
            info!(agent = %agent_name, "stopping agent");
            if let Err(e) = agent.handle.close().await {
                warn!(agent = %agent_name, err = %e, "error closing handle");
            }
            // End any active trace run.
            if let Some(run_id) = self.trace_store.active_run_id(agent_name) {
                let seq = self.trace_store.next_seq(agent_name);
                let ch = self.trace_store.run_channel_id(agent_name);
                let event = trace::build_trace_event(
                    run_id,
                    agent_name,
                    ch,
                    seq,
                    TraceEventKind::Error {
                        message: "Agent stopped".to_string(),
                    },
                );
                let _ = self.store.trace_sender().send(event);
                self.trace_store.end_run(agent_name);
            }
            self.store
                .update_agent_status(agent_name, AgentStatus::Inactive)?;
            let _ = self.store.update_agent_session(agent_name, None);
            activity_log::set_activity_state(
                &self.activity_logs,
                agent_name,
                "offline",
                "Process stopped",
            );
        }
        Ok(())
    }

    /// Kill process but keep status as sleeping (will auto-restart on next message).
    pub async fn sleep_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut agents = self.agents.lock().await;
        if let Some(mut agent) = agents.remove(agent_name) {
            info!(agent = %agent_name, "sleeping agent");
            if let Err(e) = agent.handle.close().await {
                warn!(agent = %agent_name, err = %e, "error closing handle for sleep");
            }
            self.store
                .update_agent_status(agent_name, AgentStatus::Sleeping)?;
            let _ = self.store.update_agent_session(agent_name, None);
            activity_log::set_activity_state(
                &self.activity_logs,
                agent_name,
                "offline",
                "Sleeping",
            );
        }
        Ok(())
    }

    /// Deliver a wakeup notification to agent stdin.
    pub async fn notify_agent(&self, agent_name: &str) -> anyhow::Result<()> {
        let mut agents = self.agents.lock().await;
        if let Some(agent) = agents.get_mut(agent_name) {
            agent.pending_notification_count += 1;
            let count = agent.pending_notification_count;

            let is_active = matches!(agent.handle.state(), AgentState::Active { .. });
            if !is_active {
                // Agent is mid-run (e.g. processing its init prompt). Spawn a
                // watchdog that polls for Active state, then fires the debounced
                // notification. Without this, a single-message DM sent during
                // the init turn would be permanently lost.
                debug!(agent = %agent_name, "agent not Active, spawning deferred notification watchdog");
                let agents_ref = self.agents.clone();
                let trace_store = self.trace_store.clone();
                let trace_tx = self.store.trace_sender();
                let name = agent_name.to_string();
                tokio::spawn(async move {
                    // Poll until Active or timeout (generous: init prompts can
                    // take up to ~60s for real LLMs on slow connections).
                    let ready_deadline =
                        tokio::time::Instant::now() + tokio::time::Duration::from_secs(120);
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        if tokio::time::Instant::now() >= ready_deadline {
                            warn!(agent = %name, "notify_agent watchdog: timed out waiting for Active state");
                            return;
                        }
                        let is_now_active = {
                            let agents = agents_ref.lock().await;
                            agents
                                .get(&name)
                                .map(|a| matches!(a.handle.state(), AgentState::Active { .. }))
                                .unwrap_or(false)
                        };
                        if is_now_active {
                            break;
                        }
                    }
                    // Agent is Active — apply the normal 3-second debounce then deliver.
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let mut agents = agents_ref.lock().await;
                    if let Some(agent) = agents.get_mut(&name) {
                        let current_count = agent.pending_notification_count;
                        if current_count == 0 || current_count != count {
                            agent.pending_notification_count = 0;
                            return;
                        }
                        agent.pending_notification_count = 0;
                        if !matches!(agent.handle.state(), AgentState::Active { .. }) {
                            debug!(agent = %name, "agent no longer Active after deferred debounce, skipping");
                            return;
                        }
                        let (run_id, _) = trace_store.ensure_run(&name);
                        let seq = trace_store.next_seq(&name);
                        let ch = trace_store.run_channel_id(&name);
                        let _ = trace_tx.send(trace::build_trace_event(
                            run_id,
                            &name,
                            ch,
                            seq,
                            TraceEventKind::Reading,
                        ));
                        let plural = if current_count > 1 { "s" } else { "" };
                        let them = if current_count > 1 { "them" } else { "it" };
                        let notification = format!(
                            "[System notification: You have {current_count} new message{plural} \
                             waiting. Call check_messages to read {them} when you're ready.]"
                        );
                        info!(agent = %name, count = current_count, "sending deferred prompt notification");
                        if let Err(e) = agent
                            .handle
                            .prompt(PromptReq {
                                text: notification,
                                attachments: vec![],
                            })
                            .await
                        {
                            warn!(agent = %name, error = %e, "failed to deliver deferred notification prompt");
                        }
                    }
                });
                return Ok(());
            }

            let agents_ref = self.agents.clone();
            let trace_store = self.trace_store.clone();
            let trace_tx = self.store.trace_sender();
            let name = agent_name.to_string();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let mut agents = agents_ref.lock().await;
                if let Some(agent) = agents.get_mut(&name) {
                    let current_count = agent.pending_notification_count;
                    if current_count == 0 || current_count != count {
                        agent.pending_notification_count = 0;
                        return;
                    }
                    agent.pending_notification_count = 0;

                    if !matches!(agent.handle.state(), AgentState::Active { .. }) {
                        debug!(agent = %name, "agent no longer Active after debounce, skipping");
                        return;
                    }

                    // Emit a Reading trace so the frontend shows "reading…"
                    let (run_id, _) = trace_store.ensure_run(&name);
                    let seq = trace_store.next_seq(&name);
                    let ch = trace_store.run_channel_id(&name);
                    let _ = trace_tx.send(trace::build_trace_event(
                        run_id,
                        &name,
                        ch,
                        seq,
                        TraceEventKind::Reading,
                    ));

                    let plural = if current_count > 1 { "s" } else { "" };
                    let them = if current_count > 1 { "them" } else { "it" };
                    let notification = format!(
                        "[System notification: You have {current_count} new message{plural} \
                         waiting. Call check_messages to read {them} when you're ready.]"
                    );
                    info!(agent = %name, count = current_count, "sending prompt notification");
                    if let Err(e) = agent
                        .handle
                        .prompt(PromptReq {
                            text: notification,
                            attachments: vec![],
                        })
                        .await
                    {
                        warn!(agent = %name, error = %e, "failed to deliver notification prompt");
                    }
                }
            });
        }

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

    #[cfg(test)]
    pub(crate) fn register_v2_driver(
        &mut self,
        runtime: AgentRuntime,
        driver: Arc<dyn RuntimeDriver>,
    ) {
        self.driver_registry.insert(runtime, driver);
    }
}

// ── v2 event forwarder ──

fn summarize_input(input: &serde_json::Value) -> String {
    if !input.is_object() {
        return String::new();
    }
    let obj = input.as_object().unwrap();
    // Common ACP patterns: return file path or command.
    for key in &["file_path", "path", "command", "query", "url"] {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn flush_thinking(
    text: &str,
    agent_name: &str,
    trace_store: &AgentTraceStore,
    trace_tx: &broadcast::Sender<TraceEvent>,
    activity_logs: &ActivityLogMap,
) {
    let preview: String = text.chars().take(200).collect();
    let preview = if text.chars().count() > 200 {
        format!("{preview}\u{2026}")
    } else {
        preview
    };
    trace!(agent = %agent_name, thought = %preview, "v2: thinking block complete");
    activity_log::push_activity(
        activity_logs,
        agent_name,
        ActivityEntry::Thinking {
            text: text.to_string(),
        },
    );
    let (run_id, _) = trace_store.ensure_run(agent_name);
    let seq = trace_store.next_seq(agent_name);
    let ch = trace_store.run_channel_id(agent_name);
    let _ = trace_tx.send(trace::build_trace_event(
        run_id,
        agent_name,
        ch,
        seq,
        TraceEventKind::Thinking {
            text: text.to_string(),
        },
    ));
}

fn flush_text(
    text: &str,
    agent_name: &str,
    trace_store: &AgentTraceStore,
    trace_tx: &broadcast::Sender<TraceEvent>,
) {
    let (run_id, _) = trace_store.ensure_run(agent_name);
    let seq = trace_store.next_seq(agent_name);
    let ch = trace_store.run_channel_id(agent_name);
    let _ = trace_tx.send(trace::build_trace_event(
        run_id,
        agent_name,
        ch,
        seq,
        TraceEventKind::Text {
            text: text.to_string(),
        },
    ));
}

fn spawn_v2_event_forwarder(
    _agent_name: String,
    mut event_rx: tokio::sync::mpsc::Receiver<DriverEvent>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
    trace_tx: broadcast::Sender<TraceEvent>,
    store: Arc<Store>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut pending_thinking = String::new();
        let mut pending_text = String::new();
        let mut last_tool_raw_name: Option<String> = None;

        while let Some(event) = event_rx.recv().await {
            match event {
                DriverEvent::SessionAttached {
                    ref key,
                    ref session_id,
                } => {
                    info!(agent = %key, session = %session_id, "v2: session attached");
                    let _ = store.update_agent_session(key, Some(session_id));
                    activity_log::set_activity_state(&activity_logs, key, "online", "Ready");
                }

                DriverEvent::Lifecycle { ref key, ref state } => match state {
                    AgentState::Starting => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            "working",
                            "Starting\u{2026}",
                        );
                    }
                    AgentState::Active { .. } => {
                        activity_log::set_activity_state(&activity_logs, key, "online", "Idle");
                    }
                    AgentState::Closed => {
                        activity_log::set_activity_state(&activity_logs, key, "offline", "Stopped");
                    }
                    _ => {}
                },

                DriverEvent::Output {
                    ref key,
                    run_id: _,
                    ref item,
                } => {
                    match item {
                        AgentEventItem::Thinking { text } => {
                            pending_thinking.push_str(text);
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                "thinking",
                                "Thinking\u{2026}",
                            );
                            continue;
                        }
                        AgentEventItem::Text { text } => {
                            if !pending_thinking.is_empty() {
                                flush_thinking(
                                    &pending_thinking,
                                    key,
                                    &trace_store,
                                    &trace_tx,
                                    &activity_logs,
                                );
                                pending_thinking.clear();
                            }
                            activity_log::push_activity(
                                &activity_logs,
                                key,
                                ActivityEntry::Text { text: text.clone() },
                            );
                            pending_text.push_str(text);
                            continue;
                        }
                        _ => {
                            if !pending_thinking.is_empty() {
                                flush_thinking(
                                    &pending_thinking,
                                    key,
                                    &trace_store,
                                    &trace_tx,
                                    &activity_logs,
                                );
                                pending_thinking.clear();
                            }
                            if !pending_text.is_empty() {
                                flush_text(&pending_text, key, &trace_store, &trace_tx);
                                pending_text.clear();
                            }
                        }
                    }

                    match item {
                        AgentEventItem::ToolCall { name, input } => {
                            info!(agent = %key, tool = %name, "v2: tool call");
                            last_tool_raw_name = Some(name.clone());
                            let tool_input = summarize_input(input);
                            activity_log::push_activity(
                                &activity_logs,
                                key,
                                ActivityEntry::ToolCall {
                                    tool_name: name.clone(),
                                    tool_input: tool_input.clone(),
                                },
                            );
                            activity_log::set_activity_state(&activity_logs, key, "working", name);
                            let (rid, _) = trace_store.ensure_run(key);
                            let seq = trace_store.next_seq(key);
                            let ch = trace_store.run_channel_id(key);
                            let _ = trace_tx.send(trace::build_trace_event(
                                rid,
                                key,
                                ch,
                                seq,
                                TraceEventKind::ToolCall {
                                    tool_name: name.clone(),
                                    tool_input,
                                },
                            ));
                        }
                        AgentEventItem::ToolResult { content } => {
                            let tool_name = last_tool_raw_name.clone().unwrap_or_default();
                            activity_log::upsert_tool_result_activity(
                                &activity_logs,
                                key,
                                tool_name.clone(),
                                content.clone(),
                            );
                            let (rid, _) = trace_store.ensure_run(key);
                            let seq = trace_store.next_seq(key);
                            let ch = trace_store.run_channel_id(key);
                            let _ = trace_tx.send(trace::build_trace_event(
                                rid,
                                key,
                                ch,
                                seq,
                                TraceEventKind::ToolResult {
                                    tool_name,
                                    content: content.clone(),
                                },
                            ));
                        }
                        AgentEventItem::TurnEnd => {
                            if let Some(run_id) = trace_store.active_run_id(key) {
                                let seq = trace_store.next_seq(key);
                                let ch = trace_store.run_channel_id(key);
                                let _ = trace_tx.send(trace::build_trace_event(
                                    run_id,
                                    key,
                                    ch,
                                    seq,
                                    TraceEventKind::TurnEnd,
                                ));
                                trace_store.end_run(key);
                            }
                            activity_log::set_activity_state(&activity_logs, key, "online", "Idle");
                        }
                        // Thinking/Text handled above via continue.
                        _ => {}
                    }
                }

                DriverEvent::Completed {
                    ref key,
                    run_id: _,
                    ref result,
                } => {
                    if !pending_thinking.is_empty() {
                        flush_thinking(
                            &pending_thinking,
                            key,
                            &trace_store,
                            &trace_tx,
                            &activity_logs,
                        );
                        pending_thinking.clear();
                    }
                    if !pending_text.is_empty() {
                        flush_text(&pending_text, key, &trace_store, &trace_tx);
                        pending_text.clear();
                    }
                    info!(agent = %key, reason = ?result.finish_reason, "v2: run completed");
                    if !result.session_id.is_empty() {
                        let _ = store.update_agent_session(key, Some(&result.session_id));
                    }
                    if let Some(rid) = trace_store.active_run_id(key) {
                        let seq = trace_store.next_seq(key);
                        let ch = trace_store.run_channel_id(key);
                        let _ = trace_tx.send(trace::build_trace_event(
                            rid,
                            key,
                            ch,
                            seq,
                            TraceEventKind::TurnEnd,
                        ));
                        trace_store.end_run(key);
                    }
                    activity_log::set_activity_state(&activity_logs, key, "online", "Idle");
                }

                DriverEvent::Failed {
                    ref key,
                    run_id: _,
                    ref error,
                } => {
                    let msg = format!("{error:?}");
                    error!(agent = %key, error = %msg, "v2: run failed");
                    if let Some(rid) = trace_store.active_run_id(key) {
                        let seq = trace_store.next_seq(key);
                        let ch = trace_store.run_channel_id(key);
                        let _ = trace_tx.send(trace::build_trace_event(
                            rid,
                            key,
                            ch,
                            seq,
                            TraceEventKind::Error {
                                message: msg.clone(),
                            },
                        ));
                        trace_store.end_run(key);
                    }
                    activity_log::set_activity_state(&activity_logs, key, "error", &msg);
                }
            }
        }
    })
}

// ── v2 prompt builder ──

fn build_v2_start_prompt(
    display_name: &str,
    is_resume: bool,
    unread_summary: &std::collections::HashMap<String, i64>,
    wake_message: Option<&ReceivedMessage>,
) -> String {
    if let Some(msg) = wake_message {
        let target = format_message_target(msg);
        let attachment_count = msg.attachments.as_ref().map_or(0, Vec::len);
        let content_preview = truncate_prompt_text(&msg.content, 2_000);

        let mut prompt = format!(
            "You were just woken by a new unread message.\n\
             Treat this preview as wake-up context only. The message is still unread.\n\
             Call check_messages() now to load unread messages before you respond.\n\n\
             Triggering message:\n\
             - From: {}\n\
             - Target: {target}\n\
             - Timestamp: {}\n\
             - Attachments: {attachment_count}\n\
             - Content:\n{content_preview}",
            msg.sender_name, msg.timestamp,
        );

        if !unread_summary.is_empty() {
            prompt.push_str("\n\nUnread summary:");
            for (ch, count) in unread_summary {
                prompt.push_str(&format!("\n- {ch}: {count} unread"));
            }
        }
        return prompt;
    }

    if !is_resume {
        return format!("Hello {display_name}, you are now online.");
    }

    if !unread_summary.is_empty() {
        let mut prompt = String::from("You have unread messages from while you were offline:");
        for (ch, count) in unread_summary {
            prompt.push_str(&format!("\n- {ch}: {count} unread"));
        }
        prompt.push_str(
            "\n\nUse read_history to catch up on important channels, \
             then stop. New messages will be delivered to you automatically.",
        );
        prompt
    } else {
        "No new messages while you were away. Nothing to do right now \u{2014} just stop."
            .to_string()
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

    fn active_run_id(&self, agent_name: &str) -> Option<String> {
        self.trace_store.active_run_id(agent_name)
    }

    fn set_run_channel(&self, agent_name: &str, channel_id: &str) {
        self.trace_store.set_run_channel(agent_name, channel_id);
    }
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
    use crate::agent::drivers::v2::fake::FakeDriver as FakeV2Driver;
    use crate::store::AgentRecordUpsert;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn make_test_manager(store: Arc<Store>, dir: &std::path::Path) -> AgentManager {
        let mut manager = AgentManager::new(
            store,
            dir.join("agents"),
            "chorus".to_string(),
            "http://127.0.0.1:3001".to_string(),
        );
        let fake = Arc::new(FakeV2Driver::new(AgentRuntime::Codex));
        manager.register_v2_driver(AgentRuntime::Codex, fake);
        manager
    }

    fn insert_codex_agent(store: &Store) {
        store
            .create_agent_record(&AgentRecordUpsert {
                name: "v2bot",
                display_name: "V2 Bot",
                description: Some("Test v2 agent"),
                system_prompt: None,
                runtime: "codex",
                model: "gpt-5.4-mini",
                reasoning_effort: None,
                env_vars: &[],
            })
            .unwrap();
    }

    #[tokio::test]
    async fn start_agent_with_fake_driver() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_test_manager(store, dir.path());

        let result = manager.start_agent("v2bot", None).await;

        assert!(result.is_ok(), "start_agent should succeed: {result:?}");

        let agents = manager.agents.lock().await;
        assert!(agents.contains_key("v2bot"), "v2bot should be in agents");

        // Should also show up in running agent names.
        drop(agents);
        let names = manager.get_running_agent_names().await;
        assert!(names.contains(&"v2bot".to_string()));

        // Cleanup
        let _ = manager.stop_agent("v2bot").await;
    }

    #[tokio::test]
    async fn stop_agent_idempotent() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_test_manager(store, dir.path());

        manager.start_agent("v2bot", None).await.unwrap();

        // First stop should succeed.
        let r1 = manager.stop_agent("v2bot").await;
        assert!(r1.is_ok(), "first stop should succeed: {r1:?}");

        // Second stop should also be Ok (idempotent).
        let r2 = manager.stop_agent("v2bot").await;
        assert!(r2.is_ok(), "second stop should be idempotent: {r2:?}");

        let agents = manager.agents.lock().await;
        assert!(
            !agents.contains_key("v2bot"),
            "v2bot should be removed after stop"
        );
    }

    #[tokio::test]
    async fn sleep_agent_marks_sleeping() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_test_manager(store.clone(), dir.path());

        manager.start_agent("v2bot", None).await.unwrap();

        manager.sleep_agent("v2bot").await.unwrap();

        let agents = manager.agents.lock().await;
        assert!(
            !agents.contains_key("v2bot"),
            "v2bot should be removed after sleep"
        );

        let agent_record = store.get_agent("v2bot").unwrap().unwrap();
        assert_eq!(
            agent_record.status,
            AgentStatus::Sleeping,
            "agent status should be sleeping"
        );
    }

    #[tokio::test]
    async fn start_is_noop_when_already_running() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_test_manager(store, dir.path());

        manager.start_agent("v2bot", None).await.unwrap();

        // Second start should be a no-op (returns Ok).
        let r2 = manager.start_agent("v2bot", None).await;

        assert!(r2.is_ok(), "duplicate start should be no-op: {r2:?}");

        let _ = manager.stop_agent("v2bot").await;
    }

    #[tokio::test]
    async fn notify_returns_ok_for_agent() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_test_manager(store, dir.path());

        manager.start_agent("v2bot", None).await.unwrap();

        let result = manager.notify_agent("v2bot").await;
        assert!(result.is_ok(), "notify should succeed: {result:?}");

        let _ = manager.stop_agent("v2bot").await;
    }

    // ── bridge endpoint helper ──

    fn bridge_endpoint_from(
        info: Option<crate::bridge::discovery::BridgeInfo>,
    ) -> Option<String> {
        info.map(|i| format!("http://127.0.0.1:{}", i.port))
    }

    #[test]
    fn bridge_endpoint_from_info_formats_url() {
        let info = crate::bridge::discovery::BridgeInfo {
            port: 4321,
            pid: 12345,
            started_at: "2026-04-16T00:00:00Z".to_string(),
        };
        let result = bridge_endpoint_from(Some(info));
        assert_eq!(result, Some("http://127.0.0.1:4321".to_string()));
    }

    #[test]
    fn bridge_endpoint_from_none_is_none() {
        assert_eq!(bridge_endpoint_from(None), None);
    }

    #[tokio::test]
    async fn driver_registry_has_all_runtimes() {
        let registry = build_driver_registry();
        assert_eq!(registry.len(), 4, "should have all four runtimes");
        for rt in [
            AgentRuntime::Claude,
            AgentRuntime::Codex,
            AgentRuntime::Kimi,
            AgentRuntime::Opencode,
        ] {
            assert!(
                registry.contains_key(&rt),
                "registry should contain {:?}",
                rt
            );
        }
    }
}
