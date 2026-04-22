use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use crate::agent::activity_log::{
    self, ActivityEntry, ActivityLogMap, ActivityLogResponse, ACTIVITY_OFFLINE, ACTIVITY_WORKING,
};
use crate::agent::drivers::claude::ClaudeDriver;
use crate::agent::drivers::codex::CodexDriver;
use crate::agent::drivers::kimi::KimiDriver;
use crate::agent::drivers::opencode::OpencodeDriver;
use crate::agent::drivers::{
    AgentSpec, ProcessState, PromptReq, RuntimeDriver, Session, SessionIntent,
};
use crate::agent::trace::{self, AgentTraceStore, TraceEvent, TraceEventKind};
use crate::agent::AgentLifecycle;
use crate::agent::AgentRuntime;
use crate::store::messages::ReceivedMessage;
use crate::store::Store;

/// Managed agent backed by a [`RuntimeDriver`] + [`Session`].
///
/// Visible to the sibling `event_forwarder` module (not exported beyond
/// the `agent` crate) because the forwarder's `Completed` arm and the
/// notify-agent debounce task both need to touch the handle and the
/// pending-count field. No other module should reach into these fields;
/// prefer `deliver_pending_notification` for the shared delivery path.
pub(super) struct ManagedAgent {
    pub(super) handle: Box<dyn Session>,
    pub(super) _event_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Debounce counter for stdin-style notification batching.
    pub(super) pending_notification_count: u32,
}

impl ManagedAgent {
    /// Emit a `Reading` trace event and deliver a synthetic notification
    /// prompt telling the agent how many messages arrived while it was
    /// busy. Resets `pending_notification_count` as a side effect.
    ///
    /// Called from two paths that previously duplicated this logic:
    ///   1. `notify_agent`'s debounce task (an agent is already Active,
    ///      3s of quiet elapsed since the last notification bump).
    ///   2. The event forwarder's `Completed` arm (an agent just finished
    ///      a turn and we see pending notifications queued up).
    ///
    /// Returns the number of notifications that were merged into the
    /// prompt (0 if nothing was pending; prompt is not sent in that case),
    /// or an error if the prompt dispatch itself failed.
    ///
    /// Note: the caller typically holds the manager-wide `agents` mutex
    /// while `await`ing this — meaning the prompt dispatch serializes all
    /// other manager operations. Acceptable for single-user chorus today;
    /// would want a per-agent lock if we ever scale the agent count.
    pub(super) async fn deliver_pending_notification(
        &mut self,
        trace_store: &AgentTraceStore,
        trace_tx: &broadcast::Sender<TraceEvent>,
        agent_name: &str,
    ) -> anyhow::Result<u32> {
        let count = self.pending_notification_count;
        if count == 0 {
            return Ok(0);
        }
        self.pending_notification_count = 0;
        trace::emit_event(trace_store, trace_tx, agent_name, TraceEventKind::Reading);
        let plural = if count > 1 { "s" } else { "" };
        let them = if count > 1 { "them" } else { "it" };
        let text = format!(
            "[System notification: You have {count} new message{plural} \
             waiting. Call check_messages to read {them} when you're ready.]"
        );
        self.handle
            .prompt(PromptReq {
                text,
                attachments: vec![],
            })
            .await?;
        Ok(count)
    }
}

pub struct AgentManager {
    /// Driver registry — maps runtime to its driver.
    driver_registry: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>,
    /// Active agents keyed by agent name.
    agents: Arc<Mutex<HashMap<String, ManagedAgent>>>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
    store: Arc<Store>,
    data_dir: PathBuf,
    /// Test-only override for the bridge endpoint. Production code leaves this
    /// `None` and discovery reads `~/.chorus/bridge.json`; tests set it to a
    /// synthetic URL so they don't depend on a real bridge being up.
    bridge_endpoint_override: Option<String>,
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
    pub fn new(store: Arc<Store>, data_dir: PathBuf) -> Self {
        Self {
            driver_registry: build_driver_registry(),
            agents: Arc::new(Mutex::new(HashMap::new())),
            activity_logs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            trace_store: Arc::new(AgentTraceStore::new()),
            store,
            data_dir,
            bridge_endpoint_override: None,
        }
    }

    /// Start an agent process. Creates the workspace, writes `MEMORY.md`, and
    /// optionally threads through the message that caused the wake-up.
    // TODO(per-agent-lock): start_agent races with concurrent stop/sleep/start.
    // See plan task 3.0 for documented interleavings. Fix deferred.
    pub async fn start_agent(
        &self,
        agent_name: &str,
        wake_message: Option<ReceivedMessage>,
    ) -> anyhow::Result<()> {
        // Already running? Inspect the existing handle's state — only
        // bail if it's truly live. Closed/Failed/Idle handles get evicted
        // so the recovery path can spin a fresh process. The eviction set
        // matches the "not really running" mapping in derive_status:
        //   - Closed/Idle map to Status::Asleep.
        //   - Failed maps to Status::Failed but Phase 3 routes Failed
        //     through start_agent for one retry per inbound message.
        // Active/Starting/PromptInFlight are genuinely live; short-circuit.
        //
        // Eviction must call close() on the evicted handle — several
        // drivers (kimi, opencode) do registry/fan-out teardown in close()
        // that Drop does not do, so relying on Drop would leak state.
        // We remove from the map, drop the lock, then close() + abort
        // event tasks best-effort outside the lock so other manager
        // operations aren't blocked on teardown I/O.
        let evicted = {
            let mut agents = self.agents.lock().await;
            match agents.get(agent_name).map(|e| e.handle.process_state()) {
                Some(
                    crate::agent::drivers::ProcessState::Active { .. }
                    | crate::agent::drivers::ProcessState::PromptInFlight { .. }
                    | crate::agent::drivers::ProcessState::Starting,
                ) => return Ok(()),
                Some(
                    crate::agent::drivers::ProcessState::Idle
                    | crate::agent::drivers::ProcessState::Closed
                    | crate::agent::drivers::ProcessState::Failed(_),
                ) => {
                    debug!(agent = %agent_name, "evicting dead handle before fresh start");
                    agents.remove(agent_name)
                }
                None => None,
            }
        };
        if let Some(mut dead) = evicted {
            if let Err(err) = dead.handle.close().await {
                warn!(agent = %agent_name, err = %err, "error closing evicted handle");
            }
            for task in dead._event_tasks.drain(..) {
                task.abort();
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

        let driver = self
            .driver_registry
            .get(&rt)
            .ok_or_else(|| anyhow::anyhow!("no driver for runtime {:?}", rt))?
            .clone();

        // Discover the shared bridge. `chorus serve` starts it in-process, so
        // if it's missing the server didn't boot correctly — fail loudly
        // rather than silently falling back to a (now-deleted) stdio path.
        let bridge_endpoint = self.resolve_bridge_endpoint()?;

        info!(agent = %agent_name, endpoint = %bridge_endpoint, "starting agent via shared bridge");

        let spec = AgentSpec {
            display_name: agent.display_name.clone(),
            description: agent.description.clone(),
            system_prompt: agent.system_prompt.clone(),
            model: agent.model.clone(),
            reasoning_effort: agent.reasoning_effort.clone(),
            env_vars: agent.env_vars.clone(),
            working_directory: agent_data_dir.clone(),
            bridge_endpoint,
        };

        let active = self.store.get_active_session(&agent.id)?;
        let is_resume = active.is_some();
        let intent = match active {
            Some(s) => SessionIntent::Resume(s.session_id),
            None => SessionIntent::New,
        };
        let attach_result = driver
            .open_session(agent_name.to_string(), spec, intent)
            .await?;
        let mut handle = attach_result.session;
        let events = attach_result.events;
        let event_rx = events.subscribe(); // subscribe BEFORE run
        let unread_summary = self.store.get_unread_summary(agent_name)?;
        let init_prompt_text = build_start_prompt(
            &agent.display_name,
            is_resume,
            &unread_summary,
            wake_message.as_ref(),
        );

        activity_log::set_activity_state(
            &self.activity_logs,
            agent_name,
            ACTIVITY_WORKING,
            "Starting\u{2026}",
        );
        activity_log::push_activity(
            &self.activity_logs,
            agent_name,
            ActivityEntry::Start { is_resume },
        );

        handle
            .run(Some(PromptReq {
                text: init_prompt_text,
                attachments: vec![],
            }))
            .await?;

        let forwarder = super::event_forwarder::spawn_event_forwarder(
            event_rx,
            self.activity_logs.clone(),
            self.trace_store.clone(),
            self.store.trace_sender(),
            self.store.clone(),
            self.agents.clone(),
        );

        {
            let mut agents = self.agents.lock().await;
            agents.insert(
                agent_name.to_string(),
                ManagedAgent {
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
            trace::emit_active_event(
                &self.trace_store,
                &self.store.trace_sender(),
                agent_name,
                TraceEventKind::Error {
                    message: "Agent stopped".to_string(),
                },
            );
            self.trace_store.end_run(agent_name);
            activity_log::set_activity_state(
                &self.activity_logs,
                agent_name,
                ACTIVITY_OFFLINE,
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
            activity_log::set_activity_state(
                &self.activity_logs,
                agent_name,
                ACTIVITY_OFFLINE,
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

            let is_active = matches!(agent.handle.process_state(), ProcessState::Active { .. });
            if !is_active {
                // Agent is mid-run (e.g. init turn or processing another message).
                // The event forwarder will deliver the notification immediately
                // when the current turn's Completed event fires — no polling needed.
                debug!(agent = %agent_name, "agent not Active, notification queued for post-turn delivery");
                return Ok(());
            }

            let agents_ref = self.agents.clone();
            let trace_store = self.trace_store.clone();
            let trace_tx = self.store.trace_sender();
            let name = agent_name.to_string();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let mut agents = agents_ref.lock().await;
                let Some(agent) = agents.get_mut(&name) else {
                    return;
                };
                if agent.pending_notification_count != count {
                    // Another notification bumped the count after we spawned;
                    // the newer debounce task will be authoritative. Bow out.
                    return;
                }
                if !matches!(agent.handle.process_state(), ProcessState::Active { .. }) {
                    debug!(agent = %name, "agent no longer Active after debounce, skipping");
                    agent.pending_notification_count = 0;
                    return;
                }
                match agent
                    .deliver_pending_notification(&trace_store, &trace_tx, &name)
                    .await
                {
                    Ok(delivered) if delivered > 0 => {
                        info!(agent = %name, count = delivered, "sent prompt notification");
                    }
                    Ok(_) => {} // nothing pending
                    Err(e) => {
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

    /// Returns the runtime [`ProcessState`] for `agent_name` if a process is
    /// currently managed, else `None`. Single source of truth for runtime
    /// liveness; replaces reads of any persisted column in subsequent tasks.
    pub async fn process_state(
        &self,
        agent_name: &str,
    ) -> Option<crate::agent::drivers::ProcessState> {
        let agents = self.agents.lock().await;
        agents.get(agent_name).map(|m| m.handle.process_state())
    }

    /// Test-only constructor: builds an [`AgentManager`] with an empty driver
    /// registry and a synthetic bridge endpoint override, so no real runtimes
    /// or bridge process are required. Register drivers explicitly after
    /// construction via [`register_driver`] if the test needs to start agents.
    pub fn new_for_test(store: Arc<Store>, data_dir: std::path::PathBuf) -> Self {
        let mut mgr = AgentManager::new(store, data_dir);
        mgr.bridge_endpoint_override = Some("http://127.0.0.1:1".to_string());
        mgr
    }

    /// Resolve the shared bridge endpoint. Fails loudly if no bridge is
    /// running — there is no stdio fallback anymore.
    fn resolve_bridge_endpoint(&self) -> anyhow::Result<String> {
        if let Some(override_url) = &self.bridge_endpoint_override {
            return Ok(override_url.clone());
        }
        crate::bridge::discovery::read_bridge_info()
            .map(|info| format!("http://127.0.0.1:{}", info.port))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Shared MCP bridge is not running. This usually means `chorus serve` \
                     didn't start it — this shouldn't happen. Check server logs for \
                     bridge startup errors."
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn register_driver(
        &mut self,
        runtime: AgentRuntime,
        driver: Arc<dyn RuntimeDriver>,
    ) {
        self.driver_registry.insert(runtime, driver);
    }

    #[cfg(test)]
    pub(crate) fn set_bridge_endpoint_override(&mut self, url: impl Into<String>) {
        self.bridge_endpoint_override = Some(url.into());
    }

    /// Test-only: insert a pre-built [`Session`] handle into the agent map so
    /// tests can verify eviction behaviour without going through `start_agent`'s
    /// full driver path. The inserted `ManagedAgent` has no event tasks and a
    /// zero pending-notification count — sufficient for eviction tests.
    #[doc(hidden)]
    pub async fn inject_session_for_test(
        &self,
        agent_name: impl Into<String>,
        handle: Box<dyn crate::agent::drivers::Session>,
    ) {
        let mut agents = self.agents.lock().await;
        agents.insert(
            agent_name.into(),
            ManagedAgent {
                handle,
                _event_tasks: vec![],
                pending_notification_count: 0,
            },
        );
    }
}

// The event forwarder (driver events → trace/activity/store) now lives in
// `super::event_forwarder`. Kept out of this file so the 270-line fan-out
// doesn't sit next to the lifecycle orchestration it runs alongside.

// ── prompt builder ──

fn build_start_prompt(
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
        return format!(
            "Hello {display_name}, you are now online. \
             There are no messages or tasks right now. \
             Do not use any tools. \
             Simply reply with a short acknowledgement and stop."
        );
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

    fn process_state<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<crate::agent::drivers::ProcessState>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(AgentManager::process_state(self, agent_name))
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
/// prompts.
fn format_message_target(message: &ReceivedMessage) -> String {
    match message.channel_type.as_str() {
        "dm" => format!("dm:@{}", message.channel_name),
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
    use crate::agent::drivers::fake::FakeDriver;
    use crate::store::AgentRecordUpsert;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn make_test_manager(store: Arc<Store>, dir: &std::path::Path) -> AgentManager {
        let mut manager = AgentManager::new(store, dir.join("agents"));
        let fake = Arc::new(FakeDriver::new(AgentRuntime::Codex));
        manager.register_driver(AgentRuntime::Codex, fake);
        // Tests use a synthetic endpoint — the FakeDriver ignores it, but the
        // manager now insists on one since there's no stdio fallback.
        manager.set_bridge_endpoint_override("http://127.0.0.1:1");
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
        drop(agents);

        assert!(
            manager.process_state("v2bot").await.is_none(),
            "sleep_agent should remove the managed process"
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

    // ── resolve_bridge_endpoint ──

    #[test]
    fn resolve_bridge_endpoint_returns_override_when_set() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(":memory:").unwrap());
        let mut manager = AgentManager::new(store, dir.path().join("agents"));
        manager.set_bridge_endpoint_override("http://127.0.0.1:9999");
        let got = manager.resolve_bridge_endpoint().unwrap();
        assert_eq!(got, "http://127.0.0.1:9999");
    }

    #[test]
    fn resolve_bridge_endpoint_fails_loudly_without_bridge() {
        // No override set and (in this test harness) no bridge running on
        // the local machine at the default discovery path — even if one
        // were, `read_bridge_info` returns None for stale/dead PIDs, and
        // the default path is unlikely to point at a live chorus in CI.
        // Skip the assertion if a live discovery file happens to exist.
        if crate::bridge::discovery::read_bridge_info().is_some() {
            eprintln!(
                "skipping resolve_bridge_endpoint_fails_loudly_without_bridge: \
                 live bridge detected on this machine"
            );
            return;
        }
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(":memory:").unwrap());
        let manager = AgentManager::new(store, dir.path().join("agents"));
        let err = manager
            .resolve_bridge_endpoint()
            .expect_err("must fail when no override and no bridge");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Shared MCP bridge is not running"),
            "error should name the condition: {msg}"
        );
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

    // ── Task 3.0: dead-handle eviction ──────────────────────────────────────

    /// Verify that start_agent evicts a Failed handle and falls through to a
    /// fresh start rather than short-circuiting with Ok(()) as it did before
    /// this fix.
    ///
    /// We inject a FakeHandle in ProcessState::Failed into the manager's
    /// HashMap, then call start_agent. Assertions:
    ///   - The call does NOT short-circuit: it evicts the Failed handle and
    ///     reaches the driver path, resulting in ProcessState::Active.
    ///   - The old Failed entry is no longer present.
    #[tokio::test]
    async fn start_agent_replaces_failed_handle() {
        use crate::agent::drivers::fake::FakeHandle;
        use crate::agent::drivers::{AgentError, EventFanOut, ProcessState};

        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());

        store
            .create_agent_record(&AgentRecordUpsert {
                name: "recovery-bot",
                display_name: "Recovery Bot",
                description: Some("test agent for eviction"),
                system_prompt: None,
                runtime: "codex",
                model: "gpt-fake",
                reasoning_effort: None,
                env_vars: &[],
            })
            .unwrap();

        let manager = make_test_manager(store, dir.path());

        // Inject a Failed handle simulating a previous run that crashed.
        let (events, event_tx) = EventFanOut::new();
        let failed_handle = FakeHandle::new("recovery-bot".to_string(), events, event_tx)
            .with_state(ProcessState::Failed(AgentError::Transport(
                "simulated transport failure".to_string(),
            )));
        manager
            .inject_session_for_test("recovery-bot", Box::new(failed_handle))
            .await;

        // Precondition: Failed handle is present.
        let pre_state = manager.process_state("recovery-bot").await;
        assert!(
            matches!(pre_state, Some(ProcessState::Failed(_))),
            "precondition: handle should be Failed before start_agent, got {pre_state:?}",
        );

        // Before the fix this returned Ok(()) immediately (contains_key hit).
        // After the fix it must evict and restart.
        let result = manager.start_agent("recovery-bot", None).await;
        assert!(
            result.is_ok(),
            "start_agent should succeed after evicting a Failed handle: {result:?}",
        );

        // The handle must now be Active (FakeDriver completed a fresh start).
        let post_state = manager.process_state("recovery-bot").await;
        assert!(
            matches!(post_state, Some(ProcessState::Active { .. })),
            "after start_agent the handle should be Active, got {post_state:?}",
        );

        let _ = manager.stop_agent("recovery-bot").await;
    }

    /// Verify that start_agent short-circuits without eviction when the handle
    /// is genuinely live (ProcessState::Active). The injected session id must
    /// survive the call unchanged.
    #[tokio::test]
    async fn start_agent_noop_when_handle_is_active() {
        use crate::agent::drivers::fake::FakeHandle;
        use crate::agent::drivers::{EventFanOut, ProcessState};

        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());

        store
            .create_agent_record(&AgentRecordUpsert {
                name: "live-bot",
                display_name: "Live Bot",
                description: Some("test agent for no-op check"),
                system_prompt: None,
                runtime: "codex",
                model: "gpt-fake",
                reasoning_effort: None,
                env_vars: &[],
            })
            .unwrap();

        let manager = make_test_manager(store, dir.path());

        // Inject an Active handle. start_agent must NOT replace it.
        let (events, event_tx) = EventFanOut::new();
        let active_handle = FakeHandle::new("live-bot".to_string(), events, event_tx).with_state(
            ProcessState::Active {
                session_id: "existing-session".to_string(),
            },
        );
        manager
            .inject_session_for_test("live-bot", Box::new(active_handle))
            .await;

        let result = manager.start_agent("live-bot", None).await;
        assert!(
            result.is_ok(),
            "start_agent on an Active agent should be Ok: {result:?}",
        );

        // Session id must still be the one we injected — not replaced.
        let post_state = manager.process_state("live-bot").await;
        match post_state {
            Some(ProcessState::Active { ref session_id }) => {
                assert_eq!(
                    session_id, "existing-session",
                    "Active handle must not have been replaced",
                );
            }
            other => panic!("expected Active(existing-session), got {other:?}"),
        }
    }
}
