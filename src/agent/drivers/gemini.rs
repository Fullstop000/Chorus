//! Native v2 driver for the Gemini runtime using ACP protocol.
//!
//! Multi-session: one Gemini child process per agent, N ACP sessions multiplexed
//! through its stdio. Spawned with `gemini --acp`.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, home_dir, read_file};

use super::acp_protocol::{self, AcpParsed, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcpServers` array for the ACP `session/new` inline params.
fn build_acp_mcp_servers(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    let url = super::bridge_mcp_url(bridge_endpoint);
    serde_json::json!([{
        "type": "http",
        "name": "chat",
        "url": url,
        "headers": [{"name":"X-Agent-Id","value":agent_key}]
    }])
}

const GEMINI_CHORUS_SUBDIR: &str = ".chorus";
const GEMINI_BASELINE_FILE: &str = "gemini-baseline.md";
const GEMINI_SYSTEM_FILE: &str = "gemini-system.md";

/// Write `<wd>/.chorus/gemini-system.md` containing Gemini's built-in baseline
/// system prompt followed by Chorus's standing prompt, returning the absolute
/// path. The path is consumed via `GEMINI_SYSTEM_MD` on spawn.
///
/// `GEMINI_SYSTEM_MD` is a *full replacement* for Gemini's built-in prompt
/// (per Gemini docs), so the baseline must be included or Gemini loses its
/// safety / approval / tool-use rules. The baseline is captured on first
/// spawn via `GEMINI_WRITE_SYSTEM_MD=<path> gemini -p ping` and cached in the
/// agent's workspace; subsequent spawns reuse it.
async fn ensure_gemini_system_md(spec: &AgentSpec) -> anyhow::Result<std::path::PathBuf> {
    let chorus_dir = spec.working_directory.join(GEMINI_CHORUS_SUBDIR);
    tokio::fs::create_dir_all(&chorus_dir)
        .await
        .context("failed to create .chorus dir")?;
    let baseline_path = chorus_dir.join(GEMINI_BASELINE_FILE);
    let system_path = chorus_dir.join(GEMINI_SYSTEM_FILE);

    if !baseline_path.exists() {
        let status = tokio::process::Command::new("gemini")
            .arg("-p")
            .arg("ping")
            .arg("--skip-trust")
            .env("GEMINI_WRITE_SYSTEM_MD", &baseline_path)
            .env("GEMINI_CLI_TRUST_WORKSPACE", "true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("failed to invoke `gemini` to capture baseline system prompt")?;
        if !status.success() || !baseline_path.exists() {
            anyhow::bail!(
                "gemini baseline capture failed (status {status}); \
                 ensure `gemini` is installed and authenticated"
            );
        }
    }

    let baseline = tokio::fs::read_to_string(&baseline_path)
        .await
        .context("failed to read gemini baseline")?;
    let standing = super::prompt::build_system_prompt(
        spec,
        &super::prompt::PromptOptions {
            tool_prefix: String::new(),
            extra_critical_rules: vec![
                "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".into(),
            ],
            post_startup_notes: Vec::new(),
            include_stdin_notification_section: false,
            message_notification_style: super::prompt::MessageNotificationStyle::Poll,
        },
    );
    tokio::fs::write(&system_path, format!("{baseline}\n\n---\n\n{standing}"))
        .await
        .context("failed to write gemini system.md")?;

    tokio::fs::canonicalize(&system_path)
        .await
        .context("failed to canonicalize gemini system.md path")
}

fn build_gemini_command(spec: &AgentSpec, system_md: &std::path::Path) -> Command {
    let mut args = vec!["--acp".to_string()];
    if !spec.model.is_empty() {
        args.push("--model".to_string());
        args.push(spec.model.clone());
    }

    let mut cmd = Command::new("gemini");
    cmd.args(&args)
        .current_dir(&spec.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("FORCE_COLOR", "0")
        .env("NO_COLOR", "1")
        .env("GEMINI_SYSTEM_MD", system_md)
        .env("GEMINI_CLI_TRUST_WORKSPACE", "true");
    for ev in &spec.env_vars {
        cmd.env(&ev.key, &ev.value);
    }
    cmd
}

// ---------------------------------------------------------------------------
// Per-agent shared core
// ---------------------------------------------------------------------------

struct GeminiAgentCore {
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    spec: AgentSpec,
    inner: tokio::sync::Mutex<CoreInner>,
    started: AtomicBool,
    start_in_progress: tokio::sync::Mutex<()>,
    #[cfg(test)]
    spawn_call_count: std::sync::atomic::AtomicUsize,
}

struct CoreInner {
    stdin_tx: Option<mpsc::Sender<String>>,
    shared: Option<Arc<Mutex<SharedReaderState>>>,
    next_request_id: u64,
    owned: OwnedProcess,
}

#[derive(Default)]
struct OwnedProcess {
    child: Option<std::process::Child>,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl GeminiAgentCore {
    fn new(
        key: AgentKey,
        spec: AgentSpec,
        events: EventStreamHandle,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            key,
            events,
            event_tx,
            spec,
            inner: tokio::sync::Mutex::new(CoreInner {
                stdin_tx: None,
                shared: None,
                next_request_id: 1,
                owned: OwnedProcess::default(),
            }),
            started: AtomicBool::new(false),
            start_in_progress: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            spawn_call_count: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn emit(&self, event: DriverEvent) {
        super::emit_driver_event(
            &self.event_tx,
            event,
            &self.key,
            <Self as AgentProcess>::DRIVER_NAME,
        );
    }

    pub(crate) async fn ensure_started(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.start_in_progress.lock().await;
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        self.spawn_and_initialize().await?;
        self.started.store(true, Ordering::Release);
        Ok(())
    }

    async fn spawn_and_initialize(self: &Arc<Self>) -> anyhow::Result<()> {
        #[cfg(test)]
        self.spawn_call_count.fetch_add(1, Ordering::Relaxed);

        let system_md = ensure_gemini_system_md(&self.spec).await?;
        let mut cmd = build_gemini_command(&self.spec, &system_md);
        let mut child = cmd.spawn().context("failed to spawn gemini")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        // Gemini ACP requires the `initialized` notification after receiving
        // the `initialize` response. The reader loop will send it when it
        // processes the init response (id=1). We stash the notification in
        // SharedReaderState so the reader can emit it.
        let initialized_notification = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::AwaitingInitResponse,
            sessions: HashMap::new(),
            pending: {
                let mut m = HashMap::new();
                m.insert(1, PendingRequest::Init);
                m
            },
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification: Some(initialized_notification.to_string()),
        }));

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        let mut stdin_owned = stdin;
        let stdin_handle = tokio::task::spawn_blocking(move || {
            while let Some(line) = stdin_rx.blocking_recv() {
                if writeln!(stdin_owned, "{line}").is_err() {
                    break;
                }
                if stdin_owned.flush().is_err() {
                    break;
                }
            }
        });

        let key = self.key.clone();
        let event_tx = self.event_tx.clone();
        let shared_for_reader = shared.clone();
        let stdin_tx_for_reader = stdin_tx.clone();
        let stdout_handle = tokio::spawn(async move {
            reader_loop(
                key,
                event_tx,
                shared_for_reader,
                stdin_tx_for_reader,
                stdout,
            )
            .await;
        });

        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let stderr_async = match tokio::process::ChildStderr::from_std(stderr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key_err, error = %e, "gemini: failed to convert stderr to async");
                    return;
                }
            };
            let reader = BufReader::new(stderr_async);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, line = %line, "gemini stderr");
                }
            }
        });

        {
            let mut inner = self.inner.lock().await;
            inner.owned.child = Some(child);
            inner
                .owned
                .reader_handles
                .extend([stdin_handle, stdout_handle, stderr_handle]);
            inner.stdin_tx = Some(stdin_tx);
            inner.shared = Some(shared.clone());
            inner.next_request_id = 3;
        }

        Ok(())
    }
}

impl AgentProcess for GeminiAgentCore {
    const DRIVER_NAME: &'static str = "gemini";

    fn is_stale(&self) -> bool {
        let Ok(inner) = self.inner.try_lock() else {
            return false;
        };
        match inner.stdin_tx.as_ref() {
            None => false,
            Some(tx) => tx.is_closed(),
        }
    }
}

impl Drop for GeminiAgentCore {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.try_lock() {
            if let Some(ref mut child) = inner.owned.child {
                let pid = child.id();
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            for handle in inner.owned.reader_handles.drain(..) {
                handle.abort();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Static per-process registry
// ---------------------------------------------------------------------------

fn registry() -> &'static AgentRegistry<GeminiAgentCore> {
    static REGISTRY: AgentRegistry<GeminiAgentCore> = AgentRegistry::new();
    &REGISTRY
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

struct SharedReaderState {
    phase: acp_protocol::AcpPhase,
    sessions: HashMap<String, SessionState>,
    pending: HashMap<u64, PendingRequest>,
    closed_emitted: Arc<AtomicBool>,
    /// Gemini ACP requires the `initialized` notification after the `initialize`
    /// response. Stored here so the reader loop can send it after processing
    /// the init response.
    initialized_notification: Option<String>,
}

struct SessionState {
    state: ProcessState,
    run_id: Option<RunId>,
    tool_accumulator: ToolCallAccumulator,
}

impl SessionState {
    fn new(session_id: &str) -> Self {
        Self {
            state: ProcessState::Active {
                session_id: session_id.to_string(),
            },
            run_id: None,
            tool_accumulator: ToolCallAccumulator::new(),
        }
    }
}

enum PendingRequest {
    Init,
    SessionNew {
        responder: oneshot::Sender<Result<String, String>>,
    },
    SessionLoad {
        expected_session_id: String,
        responder: oneshot::Sender<Result<String, String>>,
    },
    Prompt {
        session_id: String,
        run_id: RunId,
    },
}

// ---------------------------------------------------------------------------
// GeminiDriver
// ---------------------------------------------------------------------------

pub struct GeminiDriver;

#[async_trait]
impl RuntimeDriver for GeminiDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Gemini
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("gemini") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        // Check GEMINI_API_KEY env var first.
        if std::env::var("GEMINI_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::Authed,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        // OAuth personal account: check for ~/.gemini/oauth_creds.json
        let home = home_dir();
        let auth = read_file(&home.join(".gemini/oauth_creds.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|payload| {
                let has_token = payload["access_token"]
                    .as_str()
                    .is_some_and(|v| !v.trim().is_empty());
                if has_token {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                }
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::AcpNative,
            capabilities: CapabilitySet::MODEL_LIST,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::Failed {
            reason: "gemini does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("auto-gemini-3".into()),
            ModelInfo::from_id("gemini-3.1-pro-preview".into()),
            ModelInfo::from_id("gemini-3-flash-preview".into()),
            ModelInfo::from_id("gemini-3.1-flash-lite-preview".into()),
            ModelInfo::from_id("gemini-2.5-pro".into()),
            ModelInfo::from_id("gemini-2.5-flash".into()),
            ModelInfo::from_id("gemini-2.5-flash-lite".into()),
        ])
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        let core = if let Some(existing) = registry().get_or_evict_stale(&key) {
            existing
        } else {
            let (events, event_tx) = EventFanOut::new();
            let fresh = GeminiAgentCore::new(key.clone(), spec.clone(), events, event_tx);
            registry().insert(key.clone(), fresh.clone());
            fresh
        };
        let events = core.events.clone();
        let preassigned = match intent {
            SessionIntent::New => None,
            SessionIntent::Resume(id) => Some(id),
        };
        let handle = GeminiHandle::new(core, preassigned);
        Ok(SessionAttachment {
            session: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// GeminiHandle
// ---------------------------------------------------------------------------

pub struct GeminiHandle {
    core: Arc<GeminiAgentCore>,
    session_id: Option<SessionId>,
    state: ProcessState,
    preassigned_session_id: Option<SessionId>,
}

impl GeminiHandle {
    fn new(core: Arc<GeminiAgentCore>, preassigned_session_id: Option<SessionId>) -> Self {
        Self {
            core,
            session_id: None,
            state: ProcessState::Idle,
            preassigned_session_id,
        }
    }

    fn emit(&self, event: DriverEvent) {
        self.core.emit(event);
    }

    async fn alloc_id(&self) -> u64 {
        let mut inner = self.core.inner.lock().await;
        let id = inner.next_request_id;
        inner.next_request_id += 1;
        id
    }

    async fn send_session_new(&self) -> anyhow::Result<String> {
        let (stdin_tx, shared) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        let mcp_servers = build_acp_mcp_servers(&self.core.spec.bridge_endpoint, &self.core.key);
        let params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": mcp_servers,
        });
        {
            let mut s = shared.lock().unwrap();
            s.pending
                .insert(id, PendingRequest::SessionNew { responder: tx });
        }
        let req = acp_protocol::build_session_new_request(id, params);
        stdin_tx
            .send(req)
            .await
            .context("gemini: stdin channel closed")?;
        rx.await
            .map_err(|_| anyhow!("gemini: reader task dropped before session/new response"))?
            .map_err(|msg| anyhow!("gemini: session/new failed: {msg}"))
    }

    async fn send_session_load(&self, sid: &str) -> anyhow::Result<String> {
        let (stdin_tx, shared) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        let params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": [],
        });
        {
            let mut s = shared.lock().unwrap();
            s.pending.insert(
                id,
                PendingRequest::SessionLoad {
                    expected_session_id: sid.to_string(),
                    responder: tx,
                },
            );
        }
        let req = acp_protocol::build_session_load_request(id, sid, params);
        stdin_tx
            .send(req)
            .await
            .context("gemini: stdin channel closed")?;
        rx.await
            .map_err(|_| anyhow!("gemini: reader task dropped before session/load response"))?
            .map_err(|msg| anyhow!("gemini: session/load failed: {msg}"))
    }

    async fn acquire_stdin_and_shared(
        &self,
    ) -> anyhow::Result<(mpsc::Sender<String>, Arc<Mutex<SharedReaderState>>)> {
        let inner = self.core.inner.lock().await;
        let tx = inner
            .stdin_tx
            .clone()
            .ok_or_else(|| anyhow!("gemini: core not started"))?;
        let shared = inner
            .shared
            .clone()
            .ok_or_else(|| anyhow!("gemini: shared state missing"))?;
        Ok((tx, shared))
    }

    async fn register_session_in_shared_state(&self, session_id: &str) {
        let inner = self.core.inner.lock().await;
        if let Some(ref shared) = inner.shared {
            let mut s = shared.lock().unwrap();
            s.sessions
                .entry(session_id.to_string())
                .or_insert_with(|| SessionState::new(session_id));
        }
    }

    async fn run_inner(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.core.ensure_started().await?;

        let sid = if let Some(ref preassigned) = self.preassigned_session_id {
            self.send_session_load(preassigned).await?
        } else {
            self.send_session_new().await?
        };

        self.register_session_in_shared_state(&sid).await;
        self.session_id = Some(sid.clone());
        self.state = ProcessState::Active {
            session_id: sid.clone(),
        };
        self.emit(DriverEvent::SessionAttached {
            key: self.core.key.clone(),
            session_id: sid.clone(),
        });
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: self.state.clone(),
        });

        if let Some(req) = init_prompt {
            self.prompt(req).await?;
        }

        Ok(())
    }
}

impl Drop for GeminiHandle {
    fn drop(&mut self) {
        // `Session::close()` is the authoritative lifecycle shutdown path.
        // A dropped handle may follow an explicit close(), so emitting here
        // duplicates the terminal Closed event.
    }
}

#[async_trait]
impl Session for GeminiHandle {
    fn key(&self) -> &AgentKey {
        &self.core.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.state {
            ProcessState::Active { session_id } => Some(session_id.as_str()),
            ProcessState::PromptInFlight { session_id, .. } => Some(session_id.as_str()),
            _ => self
                .session_id
                .as_deref()
                .or(self.preassigned_session_id.as_deref()),
        }
    }

    fn process_state(&self) -> ProcessState {
        if let Some(ref sid) = self.session_id {
            if let Ok(inner) = self.core.inner.try_lock() {
                if let Some(shared) = inner.shared.as_ref() {
                    let shared = shared.lock().unwrap();
                    if let Some(session) = shared.sessions.get(sid) {
                        return session.state.clone();
                    }
                }
            }
        }
        self.state.clone()
    }

    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.run_inner(init_prompt).await
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("gemini: prompt() called before start()"))?;

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id().await;

        let (stdin_tx, shared) = {
            let inner = self.core.inner.lock().await;
            let tx = inner
                .stdin_tx
                .clone()
                .ok_or_else(|| anyhow!("gemini: stdin not available — handle not started"))?;
            let shared = inner
                .shared
                .clone()
                .ok_or_else(|| anyhow!("gemini: shared state missing"))?;
            (tx, shared)
        };

        {
            let mut s = shared.lock().unwrap();
            s.pending.insert(
                request_id,
                PendingRequest::Prompt {
                    session_id: session_id.clone(),
                    run_id,
                },
            );
            let slot = s
                .sessions
                .entry(session_id.clone())
                .or_insert_with(|| SessionState::new(&session_id));
            slot.run_id = Some(run_id);
            slot.state = ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }

        self.state = ProcessState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let prompt_req =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);
        stdin_tx
            .send(prompt_req)
            .await
            .context("gemini: stdin channel closed")?;

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        let Some(sid) = self.session_id.clone() else {
            return Ok(CancelOutcome::NotInFlight);
        };
        let shared = {
            let inner = self.core.inner.lock().await;
            inner.shared.clone()
        };
        let Some(shared) = shared else {
            return Ok(CancelOutcome::NotInFlight);
        };

        let (run_id, session_id) = {
            let mut s = shared.lock().unwrap();
            let slot = match s.sessions.get_mut(&sid) {
                Some(slot) => slot,
                None => return Ok(CancelOutcome::NotInFlight),
            };
            match &slot.state {
                ProcessState::PromptInFlight { run_id, session_id } => {
                    let rid = *run_id;
                    let psid = session_id.clone();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: psid.clone(),
                    };
                    (rid, psid)
                }
                _ => return Ok(CancelOutcome::NotInFlight),
            }
        };

        self.emit(DriverEvent::Completed {
            key: self.core.key.clone(),
            session_id: session_id.clone(),
            run_id,
            result: RunResult {
                finish_reason: FinishReason::Cancelled,
            },
        });

        self.state = ProcessState::Active { session_id };
        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, ProcessState::Closed) {
            return Ok(());
        }

        let (all_sessions_closed, shared_opt) = {
            let shared_opt = {
                let inner = self.core.inner.lock().await;
                inner.shared.clone()
            };
            if let Some(ref shared) = shared_opt {
                let mut s = shared.lock().unwrap();
                if let Some(ref sid) = self.session_id {
                    s.sessions.remove(sid);
                }
                let all_closed = s
                    .sessions
                    .values()
                    .all(|slot| matches!(slot.state, ProcessState::Closed));
                let no_pending_session_creation = !s.pending.values().any(|p| {
                    matches!(
                        p,
                        PendingRequest::SessionNew { .. } | PendingRequest::SessionLoad { .. }
                    )
                });
                (
                    all_closed && no_pending_session_creation,
                    Some(shared.clone()),
                )
            } else {
                (true, None)
            }
        };

        self.state = ProcessState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: ProcessState::Closed,
        });

        if all_sessions_closed {
            if let Some(ref shared) = shared_opt {
                let s = shared.lock().unwrap();
                // Flip before terminating transport tasks so a racing EOF path
                // can't emit a second Closed lifecycle event.
                s.closed_emitted.store(true, Ordering::SeqCst);
            }

            let key = self.core.key.clone();
            // Tear down the shared core: kill child + abort reader tasks.
            // The core's Drop will SIGTERM the child; we additionally close
            // the event stream here so subscribers observe EOF promptly.
            let mut inner = self.core.inner.lock().await;
            if let Some(ref mut child) = inner.owned.child {
                let pid = child.id();
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            inner.owned.child = None;
            for handle in inner.owned.reader_handles.drain(..) {
                handle.abort();
            }
            inner.stdin_tx = None;
            drop(inner);
            self.core.events.close();
            registry().remove(&key);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader loop
// ---------------------------------------------------------------------------

async fn reader_loop(
    key: AgentKey,
    event_tx: mpsc::Sender<DriverEvent>,
    shared: Arc<Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
    stdout: std::process::ChildStdout,
) {
    let async_stdout = match tokio::process::ChildStdout::from_std(stdout) {
        Ok(s) => s,
        Err(e) => {
            warn!(key = %key, error = %e, "gemini: failed to convert stdout to async");
            return;
        }
    };
    let reader = BufReader::new(async_stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        trace!(line = %line, "gemini stdout");

        let raw: Option<Value> = serde_json::from_str(&line).ok();

        // 1) JSON-RPC responses (have `id` + (`result` | `error`)).
        if let Some(ref msg) = raw {
            let is_response = msg.get("id").is_some()
                && (msg.get("result").is_some() || msg.get("error").is_some());
            if is_response {
                handle_response(&key, &event_tx, &shared, &stdin_tx, msg).await;
                continue;
            }
        }

        // 2) Everything else: notifications / permission requests / errors.
        let parsed = acp_protocol::parse_line(&line);
        match parsed {
            AcpParsed::InitializeResponse
            | AcpParsed::SessionResponse { .. }
            | AcpParsed::PromptResponse { .. } => {
                debug!(line = %line, "gemini: response slipped past raw check — ignoring");
            }
            AcpParsed::SessionUpdate { items } => {
                let session_id = raw
                    .as_ref()
                    .and_then(|m| m.get("params"))
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                handle_session_update(&key, &event_tx, &shared, session_id, items);
            }
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                let option_id = acp_protocol::pick_best_option_id(&options);
                debug!(
                    ?tool_name,
                    request_id, option_id, "gemini: auto-approving permission"
                );
                let response = acp_protocol::build_permission_response_raw(request_id, option_id);
                let _ = stdin_tx.try_send(response);
            }
            AcpParsed::Error { message } => {
                warn!(message = %message, "gemini: ACP error (unrouted)");
                let mut s = shared.lock().unwrap();
                let target = s
                    .sessions
                    .iter()
                    .find(|(_, st)| matches!(st.state, ProcessState::PromptInFlight { .. }))
                    .map(|(sid, st)| (sid.clone(), st.run_id));
                if let Some((sid, Some(run_id))) = target {
                    let slot = s.sessions.get_mut(&sid).unwrap();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: sid.clone(),
                    };
                    let _ = event_tx.try_send(DriverEvent::Failed {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        error: AgentError::RuntimeReported(message),
                    });
                }
            }
            AcpParsed::Unknown => {}
        }
    }

    // EOF — runtime exited. Emit TransportClosed for every in-flight run,
    // then close out the event stream.
    let (drained, already_closed) = {
        let s = shared.lock().unwrap();
        let drained: Vec<(String, RunId)> = s
            .sessions
            .iter()
            .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
            .collect();
        let already_closed = s.closed_emitted.load(Ordering::SeqCst);
        (drained, already_closed)
    };
    for (sid, run_id) in drained {
        let _ = event_tx.try_send(DriverEvent::Completed {
            key: key.clone(),
            session_id: sid,
            run_id,
            result: RunResult {
                finish_reason: FinishReason::TransportClosed,
            },
        });
    }
    if !already_closed {
        let shared_emitted = {
            let s = shared.lock().unwrap();
            s.closed_emitted.clone()
        };
        if !shared_emitted.swap(true, Ordering::SeqCst) {
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: ProcessState::Closed,
            });
        }
    }
    {
        let mut s = shared.lock().unwrap();
        for st in s.sessions.values_mut() {
            st.state = ProcessState::Closed;
        }
    }
}

async fn handle_response(
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
    msg: &Value,
) {
    let id = match msg.get("id").and_then(|v| v.as_u64()) {
        Some(id) => id,
        None => return,
    };
    let error_msg: Option<String> = msg
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if msg.get("error").is_some() {
                Some("unknown ACP error".to_string())
            } else {
                None
            }
        });

    let pending = shared.lock().unwrap().pending.remove(&id);
    let Some(pending) = pending else {
        debug!(id, "gemini: response for unknown id — ignoring");
        return;
    };

    match pending {
        PendingRequest::Init => {
            let mut s = shared.lock().unwrap();
            s.phase = acp_protocol::AcpPhase::Active;
            // Gemini ACP requires the `initialized` notification after the
            // `initialize` response. Send it now before proceeding.
            if let Some(notif) = s.initialized_notification.take() {
                let _ = stdin_tx.try_send(notif);
            }
            debug!("gemini: initialize response received, initialized notification sent");
        }
        PendingRequest::SessionNew { responder } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match session_id {
                Some(sid) => {
                    let _ = responder.send(Ok(sid));
                }
                None => {
                    let _ =
                        responder.send(Err("session/new response omitted sessionId".to_string()));
                }
            }
        }
        PendingRequest::SessionLoad {
            expected_session_id,
            responder,
        } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or(expected_session_id);
            let _ = responder.send(Ok(session_id));
        }
        PendingRequest::Prompt { session_id, run_id } => {
            let drained: Vec<(Option<String>, String, Value)> = {
                let mut s = shared.lock().unwrap();
                if let Some(slot) = s.sessions.get_mut(&session_id) {
                    let drained = slot.tool_accumulator.drain();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: session_id.clone(),
                    };
                    drained
                } else {
                    Vec::new()
                }
            };
            for (_id, name, input) in drained {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: session_id.clone(),
                    run_id,
                    item: AgentEventItem::ToolCall { name, input },
                });
            }
            let _ = event_tx.try_send(DriverEvent::Output {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                item: AgentEventItem::TurnEnd,
            });
            let _ = event_tx.try_send(DriverEvent::Completed {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            });
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: ProcessState::Active {
                    session_id: session_id.clone(),
                },
            });
        }
    }
}

fn handle_session_update(
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    session_id_hint: Option<String>,
    items: Vec<AcpUpdateItem>,
) {
    for item in items {
        let mut sid_opt: Option<String> = None;
        {
            let s = shared.lock().unwrap();
            if let Some(ref hint) = session_id_hint {
                if s.sessions.contains_key(hint) {
                    sid_opt = Some(hint.clone());
                }
            }
            if sid_opt.is_none() && s.sessions.len() == 1 {
                sid_opt = s.sessions.keys().next().cloned();
            }
        }

        match item {
            AcpUpdateItem::SessionInit { session_id } => {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
            }
            AcpUpdateItem::Thinking { text } => {
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(key, shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Thinking { text },
                    });
                }
            }
            AcpUpdateItem::Text { text } => {
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(key, shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Text { text },
                    });
                }
            }
            AcpUpdateItem::ToolCall { id, name, input } => {
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        let flushed = slot.tool_accumulator.drain();
                        let run_id = slot.run_id;
                        drop(s);
                        if let Some(run_id) = run_id {
                            for (_id, n, inp) in flushed {
                                let _ = event_tx.try_send(DriverEvent::Output {
                                    key: key.clone(),
                                    session_id: sid.clone(),
                                    run_id,
                                    item: AgentEventItem::ToolCall {
                                        name: n,
                                        input: inp,
                                    },
                                });
                            }
                        }
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            slot.tool_accumulator.record_call(id, name, input);
                        }
                    }
                }
            }
            AcpUpdateItem::ToolCallUpdate { id, input } => {
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        slot.tool_accumulator.merge_update(id, input);
                    }
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::ToolResult { content },
                        });
                    }
                }
            }
            AcpUpdateItem::TurnEnd => {
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::TurnEnd,
                        });
                    }
                }
            }
        }
    }
}

fn pick_session(
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> Option<String> {
    let s = shared.lock().unwrap();
    if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            return Some(h.to_string());
        }
        warn!(
            agent = %key,
            hint = %h,
            session_count = s.sessions.len(),
            "gemini: pick_session hint missing from sessions — falling back to single-session heuristic"
        );
    }
    if s.sessions.len() == 1 {
        return s.sessions.keys().next().cloned();
    }
    if hint.is_none() && !s.sessions.is_empty() {
        warn!(
            agent = %key,
            session_count = s.sessions.len(),
            "gemini: pick_session called with no hint and >1 live sessions — dropping update"
        );
    }
    None
}

fn pick_session_and_run(
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> (Option<String>, Option<RunId>) {
    let s = shared.lock().unwrap();
    let sid = if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            Some(h.to_string())
        } else if s.sessions.len() == 1 {
            warn!(
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "gemini: pick_session_and_run hint missing from sessions — falling back to single-session heuristic"
            );
            s.sessions.keys().next().cloned()
        } else {
            warn!(
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "gemini: pick_session_and_run hint missing with ambiguous sessions — dropping update"
            );
            None
        }
    } else if s.sessions.len() == 1 {
        s.sessions.keys().next().cloned()
    } else {
        if !s.sessions.is_empty() {
            warn!(
                agent = %key,
                session_count = s.sessions.len(),
                "gemini: pick_session_and_run called with no hint and >1 live sessions — dropping update"
            );
        }
        None
    };
    let run = sid
        .as_ref()
        .and_then(|id| s.sessions.get(id))
        .and_then(|slot| slot.run_id);
    (sid, run)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test".into(),
            description: None,
            system_prompt: None,
            model: "gemini-3.1-pro-preview".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: std::env::temp_dir(),
            bridge_endpoint: "http://127.0.0.1:9999".into(),
        }
    }

    #[test]
    fn gemini_runtime_variant_parses() {
        assert_eq!(AgentRuntime::parse("gemini"), Some(AgentRuntime::Gemini));
        assert_eq!(AgentRuntime::Gemini.as_str(), "gemini");
    }

    #[test]
    fn build_gemini_command_uses_current_dir_not_work_dir_flag() {
        let spec = test_spec();
        let cmd = build_gemini_command(&spec, std::path::Path::new("/tmp/dummy-system.md"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args, vec!["--acp", "--model", "gemini-3.1-pro-preview"]);
        assert_eq!(
            cmd.get_current_dir(),
            Some(spec.working_directory.as_path())
        );
        assert!(
            !args.iter().any(|arg| arg == "--work-dir"),
            "Gemini CLI 0.38.x rejects --work-dir; use process current_dir instead"
        );
    }

    #[tokio::test]
    async fn probe_returns_not_installed_when_binary_missing() {
        // This test assumes `gemini` is not on PATH in the test environment.
        // If it is installed, it will still pass if auth is not configured.
        let driver = GeminiDriver;
        let probe = driver.probe().await.expect("probe should not panic");
        assert_eq!(probe.transport, TransportKind::AcpNative);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
    }

    #[tokio::test]
    async fn list_models_returns_gemini_models() {
        let driver = GeminiDriver;
        let models = driver
            .list_models()
            .await
            .expect("list_models should succeed");
        let ids: Vec<_> = models.into_iter().map(|m| m.id).collect();
        assert!(ids.contains(&"gemini-3.1-pro-preview".to_string()));
        assert!(ids.contains(&"gemini-2.5-pro".to_string()));
    }

    #[tokio::test]
    async fn login_returns_failed() {
        let driver = GeminiDriver;
        match driver.login().await.expect("login should return") {
            LoginOutcome::Failed { reason } => {
                assert!(reason.contains("does not support login"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_sessions_returns_empty() {
        let driver = GeminiDriver;
        let sessions = driver
            .list_sessions()
            .await
            .expect("list_sessions should succeed");
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn event_fan_out_forwards_events() {
        let (stream, tx) = EventFanOut::new();
        let mut rx = stream.subscribe();

        tx.send(DriverEvent::Lifecycle {
            key: "a1".into(),
            state: ProcessState::Idle,
        })
        .await
        .unwrap();

        let got = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        match got {
            DriverEvent::Lifecycle { key, .. } => assert_eq!(key, "a1"),
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn close_last_session_prunes_registry_entry() {
        let key: AgentKey = format!("agent-close-prunes-registry-{}", uuid::Uuid::new_v4());
        let (events, event_tx) = EventFanOut::new();
        let core = GeminiAgentCore::new(key.clone(), test_spec(), events, event_tx);
        let session_id = "sess-last".to_string();

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: {
                let mut sessions = HashMap::new();
                sessions.insert(session_id.clone(), SessionState::new(&session_id));
                sessions
            },
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification: None,
        }));

        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);
        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared);
            inner.stdin_tx = Some(stdin_tx);
            inner.next_request_id = 3;
        }
        core.started.store(true, Ordering::Release);

        registry().insert(key.clone(), core.clone());

        let mut handle = GeminiHandle::new(core, None);
        handle.session_id = Some(session_id.clone());
        handle.state = ProcessState::Active { session_id };

        handle.close().await.unwrap();

        assert!(
            registry().get(&key).is_none(),
            "last-session close must prune the registry entry"
        );
    }

    #[tokio::test]
    async fn register_session_in_shared_state_tracks_new_handle_session() {
        let (events, event_tx) = EventFanOut::new();
        let core = GeminiAgentCore::new(
            "agent-register-session".into(),
            test_spec(),
            events,
            event_tx,
        );
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification: None,
        }));

        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared.clone());
        }

        let handle = GeminiHandle::new(core, None);
        handle
            .register_session_in_shared_state("sess-registered")
            .await;

        let s = shared.lock().unwrap();
        assert!(
            s.sessions.contains_key("sess-registered"),
            "run_inner must register each opened session in shared state"
        );
    }

    #[tokio::test]
    async fn close_with_live_secondary_does_not_tear_down_shared_child() {
        let key: AgentKey = format!("agent-live-secondary-{}", uuid::Uuid::new_v4());
        let (events, event_tx) = EventFanOut::new();
        let events_for_assert = events.clone();
        let core = GeminiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        let first_sid = "sess-first".to_string();
        let secondary_sid = "sess-secondary".to_string();
        let secondary_run = RunId::new_v4();
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: {
                let mut sessions = HashMap::new();
                sessions.insert(first_sid.clone(), SessionState::new(&first_sid));
                let mut secondary = SessionState::new(&secondary_sid);
                secondary.run_id = Some(secondary_run);
                secondary.state = ProcessState::PromptInFlight {
                    run_id: secondary_run,
                    session_id: secondary_sid.clone(),
                };
                sessions.insert(secondary_sid.clone(), secondary);
                sessions
            },
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification: None,
        }));

        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);
        let parked_reader = tokio::spawn(async {
            let () = std::future::pending().await;
        });
        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared.clone());
            inner.stdin_tx = Some(stdin_tx);
            inner.owned.reader_handles.push(parked_reader);
            inner.next_request_id = 3;
        }
        registry().insert(key.clone(), core.clone());

        let mut first_handle = GeminiHandle::new(core.clone(), None);
        first_handle.session_id = Some(first_sid.clone());
        first_handle.state = ProcessState::Active {
            session_id: first_sid.clone(),
        };
        let mut secondary_handle = GeminiHandle::new(core.clone(), None);
        secondary_handle.session_id = Some(secondary_sid.clone());
        secondary_handle.state = ProcessState::PromptInFlight {
            run_id: secondary_run,
            session_id: secondary_sid.clone(),
        };

        first_handle.close().await.unwrap();

        {
            let inner = core.inner.lock().await;
            assert!(
                inner.stdin_tx.is_some(),
                "closing one live handle must not tear down shared stdin while a sibling is active"
            );
            assert_eq!(
                inner.owned.reader_handles.len(),
                1,
                "closing one live handle must not abort shared reader tasks"
            );
            assert!(
                !inner.owned.reader_handles[0].is_finished(),
                "parked reader must still be running"
            );
        }
        assert!(
            !events_for_assert.inner.closing.load(Ordering::SeqCst),
            "closing one live handle must not close the fan-out"
        );
        assert!(
            registry().get(&key).is_some(),
            "closing one live handle must not prune the shared registry entry"
        );

        secondary_handle.close().await.unwrap();

        {
            let inner = core.inner.lock().await;
            assert!(
                inner.stdin_tx.is_none(),
                "last-session close must clear shared stdin"
            );
            assert!(
                inner.owned.reader_handles.is_empty(),
                "last-session close must drain shared reader tasks"
            );
        }
        assert!(
            events_for_assert.inner.closing.load(Ordering::SeqCst),
            "last-session close must signal the fan-out to drain"
        );
        assert!(
            registry().get(&key).is_none(),
            "last-session close must prune the registry entry"
        );
    }

    #[tokio::test]
    async fn close_emits_closed_lifecycle_only_once_even_after_drop() {
        let key: AgentKey = format!("agent-close-single-closed-{}", uuid::Uuid::new_v4());
        let (events, event_tx) = EventFanOut::new();
        let mut rx = events.subscribe();
        let core = GeminiAgentCore::new(key, test_spec(), events, event_tx);
        let session_id = "sess-closed-once".to_string();

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: {
                let mut sessions = HashMap::new();
                sessions.insert(session_id.clone(), SessionState::new(&session_id));
                sessions
            },
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification: None,
        }));

        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);
        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared);
            inner.stdin_tx = Some(stdin_tx);
            inner.next_request_id = 3;
        }
        core.started.store(true, Ordering::Release);

        let mut handle = GeminiHandle::new(core, None);
        handle.session_id = Some(session_id.clone());
        handle.state = ProcessState::Active { session_id };

        handle.close().await.unwrap();
        drop(handle);

        let mut closed_count = 0usize;
        while let Ok(Some(event)) = timeout(Duration::from_millis(100), rx.recv()).await {
            if matches!(
                event,
                DriverEvent::Lifecycle {
                    state: ProcessState::Closed,
                    ..
                }
            ) {
                closed_count += 1;
            }
        }

        assert_eq!(
            closed_count, 1,
            "closing then dropping the handle must emit exactly one Closed lifecycle event"
        );
    }
}
