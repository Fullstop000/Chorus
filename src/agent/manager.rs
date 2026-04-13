use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use crate::agent::activity_log::{self, ActivityEntry, ActivityLogMap, ActivityLogResponse};
use crate::agent::config::AgentConfig;
use crate::agent::drivers::v2::claude::ClaudeDriver;
use crate::agent::drivers::v2::codex::CodexDriver;
use crate::agent::drivers::v2::kimi::KimiDriver;
use crate::agent::drivers::v2::opencode::OpencodeDriver;
use crate::agent::drivers::v2::v1_adapter::V1DriverAdapter;
use crate::agent::drivers::v2::{
    AgentEventItem, AgentHandle, AgentSpec, AgentState, DriverEvent, PromptReq, RuntimeDriver,
    StartOpts,
};
use crate::agent::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::agent::trace::{self, AgentTraceStore, TraceEvent, TraceEventKind};
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
    /// Accumulated text output chunks, flushed as a single trace event on the next non-Text event.
    pending_text: String,
}

/// V2-managed agent backed by a [`RuntimeDriver`] + [`AgentHandle`].
struct V2Agent {
    handle: Box<dyn AgentHandle>,
    _event_tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub struct AgentManager {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    /// v2 driver registry — maps runtime to native v2 driver OR V1DriverAdapter.
    driver_registry: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>,
    /// v2-managed agents keyed by agent name.
    v2_agents: Arc<Mutex<HashMap<String, V2Agent>>>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
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

fn build_driver_registry() -> HashMap<AgentRuntime, Arc<dyn RuntimeDriver>> {
    let v2_runtimes: HashSet<String> = std::env::var("CHORUS_DRIVER_V2_RUNTIMES")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let all_runtimes = [
        AgentRuntime::Claude,
        AgentRuntime::Codex,
        AgentRuntime::Kimi,
        AgentRuntime::Opencode,
    ];

    let mut registry: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>> = HashMap::new();

    for rt in all_runtimes {
        let driver: Arc<dyn RuntimeDriver> = if v2_runtimes.contains(rt.as_str()) {
            match rt {
                AgentRuntime::Kimi => Arc::new(KimiDriver),
                AgentRuntime::Claude => Arc::new(ClaudeDriver),
                AgentRuntime::Codex => Arc::new(CodexDriver),
                AgentRuntime::Opencode => Arc::new(OpencodeDriver),
            }
        } else {
            let v1_driver = crate::agent::drivers::driver_for_runtime(rt);
            Arc::new(V1DriverAdapter::new(v1_driver))
        };
        registry.insert(rt, driver);
    }

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
            agents: Arc::new(Mutex::new(HashMap::new())),
            driver_registry: build_driver_registry(),
            v2_agents: Arc::new(Mutex::new(HashMap::new())),
            activity_logs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            trace_store: Arc::new(AgentTraceStore::new()),
            store,
            data_dir,
            bridge_binary,
            server_url,
            next_instance_id: AtomicU64::new(1),
        }
    }

    /// Returns `true` if the given runtime maps to a native v2 driver
    /// (i.e. not a V1DriverAdapter wrapper).
    fn is_native_v2(&self, runtime: AgentRuntime) -> bool {
        std::env::var("CHORUS_DRIVER_V2_RUNTIMES")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .any(|s| s == runtime.as_str())
    }

    /// Start an agent process. Creates the workspace, writes `MEMORY.md`, and
    /// optionally threads through the message that caused the wake-up.
    pub async fn start_agent(
        &self,
        agent_name: &str,
        wake_message: Option<ReceivedMessage>,
    ) -> anyhow::Result<()> {
        // Already running in either v1 or v2?
        {
            let agents = self.agents.lock().await;
            if agents.contains_key(agent_name) {
                return Ok(());
            }
        }
        {
            let v2 = self.v2_agents.lock().await;
            if v2.contains_key(agent_name) {
                return Ok(());
            }
        }

        let agent = self
            .store
            .get_agent(agent_name)?
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {agent_name}"))?;

        let driver = get_driver(&agent.runtime)?;
        // Raw Kimi driver requires a pre-generated session id (it uses
        // stdin notifications, so the server needs a session ID up front).
        // ACP drivers handle sessions internally via session/new|load.
        let resumable_session_id = if driver.needs_pregenerated_session_id() {
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
            system_prompt: agent.system_prompt.clone(),
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

        // ── v2 path: use native RuntimeDriver when configured ──
        let runtime = AgentRuntime::parse(&agent.runtime);
        let use_v2 = runtime.map(|rt| self.is_native_v2(rt)).unwrap_or(false);

        if use_v2 {
            let rt = runtime.unwrap();
            let v2_driver = self
                .driver_registry
                .get(&rt)
                .ok_or_else(|| anyhow::anyhow!("v2: no driver for runtime {:?}", rt))?
                .clone();

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
            };

            let attach_result = v2_driver.attach(agent_name.to_string(), spec).await?;
            let mut handle = attach_result.handle;
            let events = attach_result.events;

            // Subscribe BEFORE start so we don't miss early events.
            let event_rx = events.subscribe();

            let is_resume = agent.session_id.is_some();
            let unread_summary = self.store.get_unread_summary(agent_name)?;

            // Build a simple init prompt for v2 (no v1 driver needed for system prompt — it's in the spec).
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
                let mut v2 = self.v2_agents.lock().await;
                v2.insert(
                    agent_name.to_string(),
                    V2Agent {
                        handle,
                        _event_tasks: vec![forwarder],
                    },
                );
            }

            info!(agent = %agent_name, runtime = %rt.as_str(), "v2: agent started");
            return Ok(());
        }

        // ── v1 path (existing) ──

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
                    pending_text: String::new(),
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
        // ── v2 path ──
        {
            let mut v2 = self.v2_agents.lock().await;
            if let Some(mut v2_agent) = v2.remove(agent_name) {
                info!(agent = %agent_name, "v2: stopping agent");
                if let Err(e) = v2_agent.handle.close().await {
                    warn!(agent = %agent_name, err = %e, "v2: error closing handle");
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
                return Ok(());
            }
        }

        // ── v1 path ──
        let mut running = {
            let mut agents = self.agents.lock().await;
            match agents.remove(agent_name) {
                Some(r) => r,
                None => return Ok(()),
            }
        };
        // Emit synthetic error trace if the agent had an active run.
        if let Some(run_id) = self.trace_store.active_run_id(agent_name) {
            let seq = self.trace_store.next_seq(agent_name);
            let ch = self.trace_store.run_channel_id(agent_name);
            let event = trace::build_trace_event(
                run_id,
                agent_name,
                ch,
                seq,
                TraceEventKind::Error {
                    message: "Agent restarted".to_string(),
                },
            );
            let _ = self.store.trace_sender().send(event);
            self.trace_store.end_run(agent_name);
        }
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
        // ── v2 path ──
        {
            let mut v2 = self.v2_agents.lock().await;
            if let Some(mut v2_agent) = v2.remove(agent_name) {
                info!(agent = %agent_name, "v2: sleeping agent");
                if let Err(e) = v2_agent.handle.close().await {
                    warn!(agent = %agent_name, err = %e, "v2: error closing handle for sleep");
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
                return Ok(());
            }
        }

        // ── v1 path ──
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
        // ── v2 path: notification not yet supported ──
        {
            let v2 = self.v2_agents.lock().await;
            if v2.contains_key(agent_name) {
                debug!(agent = %agent_name, "v2: notify not yet implemented, skipping");
                return Ok(());
            }
        }

        // ── v1 path ──
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
        let trace_store = self.trace_store.clone();
        let trace_tx = self.store.trace_sender();
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

                // Emit a Reading trace event so the frontend shows "reading…"
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
        let v1_names: Vec<String> = self.agents.lock().await.keys().cloned().collect();
        let v2_names: Vec<String> = self.v2_agents.lock().await.keys().cloned().collect();
        for name in v1_names.into_iter().chain(v2_names) {
            self.stop_agent(&name).await?;
        }
        Ok(())
    }

    pub async fn get_running_agent_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.agents.lock().await.keys().cloned().collect();
        let v2_names: Vec<String> = self.v2_agents.lock().await.keys().cloned().collect();
        names.extend(v2_names);
        names
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
        let trace_store = self.trace_store.clone();
        let trace_tx = store.trace_sender();
        let name = agent_name.clone();

        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            // Enter an `agent` span for the lifetime of the reader. The
            // AgentLogLayer in crate::logging routes every event in scope
            // into <logs_dir>/agents/<name>.log — no explicit file
            // handle plumbing required.
            let agent_span = tracing::info_span!(
                crate::logging::AGENT_SPAN_NAME,
                agent_name = %name,
                instance_id,
            );
            let _span_guard = agent_span.enter();
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
                // TRACE-level so default RUST_LOG doesn't flood stdout.
                // The per-layer TRACE filter on AgentLogLayer captures
                // this regardless of the user's global log level.
                tracing::trace!(stream = "stdout", raw = %line);

                for event in driver.parse_line(&line) {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        handle_parsed_event(
                            &agents,
                            &activity_logs,
                            &trace_store,
                            &trace_tx,
                            &store,
                            &name,
                            event,
                            &driver,
                        )
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

                // Emit synthetic error trace if the agent had an active run.
                if let Some(run_id) = trace_store.active_run_id(&name) {
                    let seq = trace_store.next_seq(&name);
                    let ch = trace_store.run_channel_id(&name);
                    let event = trace::build_trace_event(
                        run_id,
                        &name,
                        ch,
                        seq,
                        TraceEventKind::Error {
                            message: "Agent process exited unexpectedly".to_string(),
                        },
                    );
                    let _ = trace_tx.send(event);
                    trace_store.end_run(&name);
                }

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
            let agent_span = tracing::info_span!(
                crate::logging::AGENT_SPAN_NAME,
                agent_name = %agent_name,
                instance_id,
            );
            let _span_guard = agent_span.enter();
            let reader = std::io::BufReader::new(stderr);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        error!(err = %e, backtrace = %project_backtrace(), "stderr read error");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                // WARN because agent stderr is nearly always a problem.
                // The AgentLogLayer captures it in the per-agent log file
                // and the ExcludeAgentSpansFilter keeps it out of chorus.log.
                warn!(stream = "stderr", raw = %line);
            }
        });
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

#[allow(clippy::too_many_arguments)]
async fn handle_parsed_event(
    agents: &Arc<Mutex<HashMap<String, RunningAgent>>>,
    logs: &Arc<ActivityLogMap>,
    trace_store: &Arc<AgentTraceStore>,
    trace_tx: &broadcast::Sender<TraceEvent>,
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
            ActivityEntry::Thinking {
                text: full_thought.clone(),
            },
        );
        // Emit trace event for accumulated thinking block.
        let (run_id, _) = trace_store.ensure_run(agent_name);
        let seq = trace_store.next_seq(agent_name);
        let ch = trace_store.run_channel_id(agent_name);
        let _ = trace_tx.send(trace::build_trace_event(
            run_id,
            agent_name,
            ch,
            seq,
            TraceEventKind::Thinking { text: full_thought },
        ));
    }

    // Flush accumulated text output when a non-Text event arrives.
    let is_text_event = matches!(event, ParsedEvent::Text { .. });
    if !is_text_event && !running.pending_text.is_empty() {
        let full_text = std::mem::take(&mut running.pending_text);
        // Use ensure_run so text-only turns (no prior Thinking/ToolCall) are traced.
        let (run_id, _) = trace_store.ensure_run(agent_name);
        let seq = trace_store.next_seq(agent_name);
        let ch = trace_store.run_channel_id(agent_name);
        let _ = trace_tx.send(trace::build_trace_event(
            run_id,
            agent_name,
            ch,
            seq,
            TraceEventKind::Text { text: full_text },
        ));
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

                // Emit a Reading trace event so the frontend shows "reading…"
                let (run_id, _) = trace_store.ensure_run(agent_name);
                let seq = trace_store.next_seq(agent_name);
                let ch = trace_store.run_channel_id(agent_name);
                let _ = trace_tx.send(trace::build_trace_event(
                    run_id,
                    agent_name,
                    ch,
                    seq,
                    TraceEventKind::Reading,
                ));

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
            // Accumulate text chunks; flushed as a single trace event on next non-Text event.
            running.pending_text.push_str(text);
        }
        ParsedEvent::ToolCall {
            ref name,
            ref input,
        } => {
            let display_name = driver.tool_display_name(name);
            let tool_input = driver.summarize_tool_input(name, input);
            if tool_input.is_empty() {
                info!(agent = %agent_name, tool = %name, "tool call");
            } else {
                info!(agent = %agent_name, tool = %name, input = %tool_input, "tool call");
            }
            running.last_tool_raw_name = Some(name.clone());
            activity_log::push_activity(
                logs,
                agent_name,
                ActivityEntry::ToolCall {
                    tool_name: name.clone(),
                    tool_input: tool_input.clone(),
                },
            );
            activity_log::set_activity_state(logs, agent_name, "working", &display_name);
            // Emit trace event for tool call.
            let (run_id, _) = trace_store.ensure_run(agent_name);
            let seq = trace_store.next_seq(agent_name);
            let ch = trace_store.run_channel_id(agent_name);
            let _ = trace_tx.send(trace::build_trace_event(
                run_id,
                agent_name,
                ch,
                seq,
                TraceEventKind::ToolCall {
                    tool_name: name.clone(),
                    tool_input,
                },
            ));
        }
        ParsedEvent::ToolCallUpdate { ref input } => {
            // Deferred tool-call input from ACP `tool_call_update` with `rawInput`.
            let tool_name = running.last_tool_raw_name.clone().unwrap_or_default();
            let tool_input = driver.summarize_tool_input(&tool_name, input);
            if !tool_input.is_empty() {
                info!(agent = %agent_name, tool = %tool_name, input = %tool_input, "tool call input update");
                activity_log::update_tool_call_input(logs, agent_name, tool_input.clone());
                // Emit a corrected trace event so the frontend has the real input.
                let (run_id, _) = trace_store.ensure_run(agent_name);
                let seq = trace_store.next_seq(agent_name);
                let ch = trace_store.run_channel_id(agent_name);
                let _ = trace_tx.send(trace::build_trace_event(
                    run_id,
                    agent_name,
                    ch,
                    seq,
                    TraceEventKind::ToolCall {
                        tool_name,
                        tool_input,
                    },
                ));
            }
        }
        ParsedEvent::ToolResult { ref content } => {
            let tool_name = running.last_tool_raw_name.clone().unwrap_or_default();
            activity_log::upsert_tool_result_activity(
                logs,
                agent_name,
                tool_name.clone(),
                content.clone(),
            );
            // Emit trace event for tool result.
            let (run_id, _) = trace_store.ensure_run(agent_name);
            let seq = trace_store.next_seq(agent_name);
            let ch = trace_store.run_channel_id(agent_name);
            let _ = trace_tx.send(trace::build_trace_event(
                run_id,
                agent_name,
                ch,
                seq,
                TraceEventKind::ToolResult {
                    tool_name,
                    content: content.clone(),
                },
            ));
        }
        ParsedEvent::TurnEnd { session_id } => {
            info!(agent = %agent_name, "turn ended");
            if let Some(ref sid) = session_id {
                running.session_id = Some(sid.clone());
                let _ = store.update_agent_session(agent_name, Some(sid));
            }
            // Emit trace TurnEnd before ending the run.
            if let Some(run_id) = trace_store.active_run_id(agent_name) {
                let seq = trace_store.next_seq(agent_name);
                let ch = trace_store.run_channel_id(agent_name);
                let _ = trace_tx.send(trace::build_trace_event(
                    run_id,
                    agent_name,
                    ch,
                    seq,
                    TraceEventKind::TurnEnd,
                ));
                trace_store.end_run(agent_name);
            }
            activity_log::set_activity_state(logs, agent_name, "online", "Idle");
        }
        ParsedEvent::Error { ref message } => {
            error!(agent = %agent_name, message = %message, backtrace = %project_backtrace(), "agent error");
            // Emit trace error and end the run.
            if let Some(run_id) = trace_store.active_run_id(agent_name) {
                let seq = trace_store.next_seq(agent_name);
                let ch = trace_store.run_channel_id(agent_name);
                let _ = trace_tx.send(trace::build_trace_event(
                    run_id,
                    agent_name,
                    ch,
                    seq,
                    TraceEventKind::Error {
                        message: message.clone(),
                    },
                ));
                trace_store.end_run(agent_name);
            }
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
    use crate::store::AgentRecordUpsert;
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
            pending_text: String::new(),
        }
    }

    fn make_test_trace() -> (Arc<AgentTraceStore>, broadcast::Sender<TraceEvent>) {
        let trace_store = Arc::new(AgentTraceStore::new());
        let (trace_tx, _) = broadcast::channel(64);
        (trace_store, trace_tx)
    }

    #[tokio::test]
    async fn thinking_chunks_accumulate_and_flush_as_single_activity_entry() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        let agents: Arc<Mutex<HashMap<String, RunningAgent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let logs: Arc<ActivityLogMap> = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let driver: Arc<dyn Driver> = Arc::new(FakeDriver);
        let (trace_store, trace_tx) = make_test_trace();

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        // Feed three thinking chunks — should not appear in the log yet.
        for chunk in &["Let me ", "think ", "carefully."] {
            handle_parsed_event(
                &agents,
                &logs,
                &trace_store,
                &trace_tx,
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
            &trace_store,
            &trace_tx,
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
        let (trace_store, trace_tx) = make_test_trace();

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        for chunk in &["Hello ", "world", "!"] {
            handle_parsed_event(
                &agents,
                &logs,
                &trace_store,
                &trace_tx,
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
        let (trace_store, trace_tx) = make_test_trace();

        agents
            .lock()
            .await
            .insert("bot1".to_string(), make_running_agent(driver.clone()));

        // Feed two thinking chunks then a ToolCall.
        for chunk in &["step one ", "step two"] {
            handle_parsed_event(
                &agents,
                &logs,
                &trace_store,
                &trace_tx,
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
            &trace_store,
            &trace_tx,
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
            system_prompt: None,
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
            .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
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
                    pending_text: String::new(),
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
                    pending_text: String::new(),
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

    // ── v2 driver registry tests ──

    use crate::agent::drivers::v2::fake::FakeDriver as FakeV2Driver;

    fn make_v2_manager(store: Arc<Store>, dir: &std::path::Path) -> AgentManager {
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
    async fn v2_start_agent_with_fake_driver() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        // Force codex to use native v2 by registering FakeV2Driver directly.
        let manager = make_v2_manager(store, dir.path());

        // The FakeDriver is native v2 but is_native_v2 checks env var. Override for test.
        std::env::set_var("CHORUS_DRIVER_V2_RUNTIMES", "codex");
        let result = manager.start_agent("v2bot", None).await;
        std::env::remove_var("CHORUS_DRIVER_V2_RUNTIMES");

        assert!(result.is_ok(), "v2 start_agent should succeed: {result:?}");

        let v2 = manager.v2_agents.lock().await;
        assert!(v2.contains_key("v2bot"), "v2bot should be in v2_agents");

        // Should also show up in running agent names.
        drop(v2);
        let names = manager.get_running_agent_names().await;
        assert!(names.contains(&"v2bot".to_string()));

        // Cleanup
        let _ = manager.stop_agent("v2bot").await;
    }

    #[tokio::test]
    async fn v2_stop_agent_idempotent() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_v2_manager(store, dir.path());

        std::env::set_var("CHORUS_DRIVER_V2_RUNTIMES", "codex");
        manager.start_agent("v2bot", None).await.unwrap();
        std::env::remove_var("CHORUS_DRIVER_V2_RUNTIMES");

        // First stop should succeed.
        let r1 = manager.stop_agent("v2bot").await;
        assert!(r1.is_ok(), "first stop should succeed: {r1:?}");

        // Second stop should also be Ok (idempotent).
        let r2 = manager.stop_agent("v2bot").await;
        assert!(r2.is_ok(), "second stop should be idempotent: {r2:?}");

        let v2 = manager.v2_agents.lock().await;
        assert!(
            !v2.contains_key("v2bot"),
            "v2bot should be removed after stop"
        );
    }

    #[tokio::test]
    async fn v2_sleep_agent_marks_sleeping() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_v2_manager(store.clone(), dir.path());

        std::env::set_var("CHORUS_DRIVER_V2_RUNTIMES", "codex");
        manager.start_agent("v2bot", None).await.unwrap();
        std::env::remove_var("CHORUS_DRIVER_V2_RUNTIMES");

        manager.sleep_agent("v2bot").await.unwrap();

        let v2 = manager.v2_agents.lock().await;
        assert!(
            !v2.contains_key("v2bot"),
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
    async fn v2_start_is_noop_when_already_running() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_v2_manager(store, dir.path());

        std::env::set_var("CHORUS_DRIVER_V2_RUNTIMES", "codex");
        manager.start_agent("v2bot", None).await.unwrap();

        // Second start should be a no-op (returns Ok).
        let r2 = manager.start_agent("v2bot", None).await;
        std::env::remove_var("CHORUS_DRIVER_V2_RUNTIMES");

        assert!(r2.is_ok(), "duplicate start should be no-op: {r2:?}");

        let _ = manager.stop_agent("v2bot").await;
    }

    #[tokio::test]
    async fn v2_notify_returns_ok_for_v2_agent() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap());
        insert_codex_agent(&store);

        let manager = make_v2_manager(store, dir.path());

        std::env::set_var("CHORUS_DRIVER_V2_RUNTIMES", "codex");
        manager.start_agent("v2bot", None).await.unwrap();
        std::env::remove_var("CHORUS_DRIVER_V2_RUNTIMES");

        // notify should not error even though v2 notify is not implemented.
        let result = manager.notify_agent("v2bot").await;
        assert!(result.is_ok(), "notify should succeed: {result:?}");

        let _ = manager.stop_agent("v2bot").await;
    }

    #[tokio::test]
    async fn driver_registry_defaults_to_v1_adapters() {
        let registry = build_driver_registry();
        assert_eq!(registry.len(), 4, "should have all four runtimes");
        // Without CHORUS_DRIVER_V2_RUNTIMES set, all should be V1DriverAdapter.
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
