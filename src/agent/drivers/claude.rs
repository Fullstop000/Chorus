//! v2 Claude driver backed by Claude headless CLI.
//!
//! # Multi-session model: **process-per-session**
//!
//! Claude's `claude -p --output-format stream-json` is a per-invocation
//! command — each child process exits after its turn completes and cannot
//! multiplex multiple sessions like Codex's `thread/start` or Kimi/OpenCode's
//! `session/new` do. Phase 0.9 Stage 2 therefore runs **one `tokio::process::Child`
//! per [`AgentSessionHandle`]**.
//!
//! What's shared across an agent's sessions:
//!
//! - [`EventStreamHandle`] / `event_tx` — every child under one agent writes
//!   events into the same fan-out so subscribers see a single timeline.
//! - The process-global [`agent_instances`] registry — `attach`,
//!   `new_session`, `resume_session` all route through [`ClaudeDriver::ensure_process`]
//!   to reach the same [`ClaudeAgentProcess`] for a given [`AgentKey`].
//!
//! What's per-handle (unique to Claude among the four runtimes):
//!
//! - The `claude -p` child itself.
//! - The stdin writer channel + reader tasks.
//! - `SharedReaderState` (session id + run id + per-session lifecycle).
//!
//! Session id discovery:
//!
//! - **New session**: the spawned child emits `system.init` with its minted
//!   `session_id` on stdout; the stdout reader captures it into
//!   [`SharedReaderState`] and emits [`DriverEvent::SessionAttached`].
//! - **Resume session**: the caller pre-supplies the id via
//!   [`RuntimeDriver::resume_session`]; `start()` passes it as
//!   `--resume <session_id>` to the CLI. The resumed child still emits
//!   `system.init` (echoing the same id), which drives the same
//!   [`DriverEvent::SessionAttached`] path.
//!
//! Registry pruning: on the bootstrap handle's `close()` the registry entry is
//! dropped so a subsequent `attach` on the same key builds a fresh
//! [`ClaudeAgentProcess`] (new `EventStreamHandle`, new `event_tx`). Secondary
//! handles prune only when they were the last live session on the agent.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::claude_headless::{self, HeadlessEvent};
use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcpServers.chat` config block for `.chorus-claude-mcp.json`.
///
/// Produces the native HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Factored out so config-shape
/// tests don't need a live bridge.
fn build_mcp_config(bridge_endpoint: &str, token: &str) -> serde_json::Value {
    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "type": "http",
                "url": url
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Handle role
// ---------------------------------------------------------------------------

/// Which slot a [`ClaudeHandle`] fills on its agent's shared process.
///
/// The `Bootstrap` handle — returned by [`RuntimeDriver::attach`] — owns the
/// agent's registry entry: its `close()` unconditionally prunes the entry so
/// the next `attach` rebuilds a fresh [`ClaudeAgentProcess`]. `Secondary`
/// handles (from `new_session` / `resume_session`) prune only if they were
/// the last live session.
///
/// Mirrors the `HandleRole` shape in `kimi.rs` and `opencode.rs` so every
/// "is this the bootstrap handle?" branch reads as intent rather than a
/// boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleRole {
    Bootstrap,
    Secondary,
}

impl HandleRole {
    #[allow(dead_code)]
    fn is_bootstrap(&self) -> bool {
        matches!(self, Self::Bootstrap)
    }
}

// ---------------------------------------------------------------------------
// Transport abstraction — lets tests inject a fake stdin/stdout pair
// ---------------------------------------------------------------------------

/// Thin abstraction over the Claude headless transport. Production uses
/// [`SpawnedClaudeTransport`] (a real `claude -p` child process); tests
/// inject a fake stdio pair to drive the reader task without a binary.
///
/// Each `ClaudeHandle::start()` spawns one of these — Claude's CLI cannot
/// multiplex, so we get one per session.
trait ClaudeTransport: Send {
    /// Take the stdout reader half. Called exactly once.
    fn take_stdout(&mut self) -> Box<dyn AsyncRead + Send + Unpin>;
    /// Take the stderr reader half, if one exists. Called at most once.
    fn take_stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>>;
    /// Take the stdin writer half. Called exactly once.
    fn take_stdin(&mut self) -> Box<dyn AsyncWrite + Send + Unpin>;
    /// Signal the underlying process to terminate. No-op for fakes.
    fn terminate(&mut self);
}

/// Transport backed by a spawned `claude -p` child process.
struct SpawnedClaudeTransport {
    child: Option<tokio::process::Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    pid: Option<u32>,
}

impl ClaudeTransport for SpawnedClaudeTransport {
    fn take_stdout(&mut self) -> Box<dyn AsyncRead + Send + Unpin> {
        Box::new(self.stdout.take().expect("stdout taken twice"))
    }
    fn take_stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.stderr
            .take()
            .map(|s| -> Box<dyn AsyncRead + Send + Unpin> { Box::new(s) })
    }
    fn take_stdin(&mut self) -> Box<dyn AsyncWrite + Send + Unpin> {
        Box::new(self.stdin.take().expect("stdin taken twice"))
    }
    fn terminate(&mut self) {
        if let Some(pid) = self.pid {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        // Drop the tokio::Child — this also signals kill-on-drop (the default
        // for tokio::process::Child is to NOT kill on drop, hence explicit
        // SIGTERM above).
        self.child = None;
    }
}

/// Factory for a transport: called once per `start()`. Production spawns a
/// real child via [`spawn_real_transport`]; tests install a fake factory via
/// [`ClaudeAgentProcess::set_transport_factory`].
type TransportFactory =
    Arc<dyn Fn(Vec<String>, &AgentSpec) -> anyhow::Result<Box<dyn ClaudeTransport>> + Send + Sync>;

fn spawn_real_transport(
    args: Vec<String>,
    spec: &AgentSpec,
) -> anyhow::Result<Box<dyn ClaudeTransport>> {
    let mut cmd = TokioCommand::new("claude");
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Env: remove CLAUDECODE (prevents nested invocation detection)
    cmd.env_remove("CLAUDECODE");
    cmd.env("FORCE_COLOR", "0");
    for ev in &spec.env_vars {
        cmd.env(&ev.key, &ev.value);
    }

    let mut child = cmd.spawn().context("failed to spawn claude")?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("claude v2: child has no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("claude v2: child has no stdout"))?;
    let stderr = child.stderr.take();

    Ok(Box::new(SpawnedClaudeTransport {
        child: Some(child),
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr,
        pid,
    }))
}

// ---------------------------------------------------------------------------
// ClaudeDriver — RuntimeDriver
// ---------------------------------------------------------------------------

/// Zero-size driver. The per-agent shared state lives in [`agent_instances`]
/// so `ClaudeDriver` stays constructible via `Arc::new(ClaudeDriver)` in
/// `manager.rs`.
pub struct ClaudeDriver;

/// Process-global registry: one [`ClaudeAgentProcess`] per [`AgentKey`].
/// Populated by `attach`; reused by subsequent `new_session` /
/// `resume_session` calls on the same key so every child under one agent
/// writes into the same [`EventStreamHandle`].
fn agent_instances() -> &'static Mutex<HashMap<AgentKey, Arc<ClaudeAgentProcess>>> {
    static INSTANCES: OnceLock<Mutex<HashMap<AgentKey, Arc<ClaudeAgentProcess>>>> =
        OnceLock::new();
    INSTANCES.get_or_init(|| Mutex::new(HashMap::new()))
}

impl ClaudeDriver {
    /// Return the existing shared process for `key`, or create one if it's
    /// the first `attach` for this agent. Evicts a stale cached entry whose
    /// `closed` flag was flipped by a prior bootstrap close so the new
    /// attach builds a fresh process.
    fn ensure_process(&self, key: &AgentKey) -> Arc<ClaudeAgentProcess> {
        let mut guard = agent_instances().lock().unwrap();
        if let Some(existing) = guard.get(key) {
            if existing.is_stale() {
                debug!(
                    agent = %key,
                    "claude: evicting stale agent process (closed) before re-attach"
                );
                guard.remove(key);
            } else {
                return Arc::clone(existing);
            }
        }
        let (events, event_tx) = EventFanOut::new();
        let proc = Arc::new(ClaudeAgentProcess {
            key: key.clone(),
            events,
            event_tx,
            closed: AtomicBool::new(false),
            live_sessions: Mutex::new(0),
            transport_factory: Mutex::new(Arc::new(spawn_real_transport)),
        });
        guard.insert(key.clone(), Arc::clone(&proc));
        proc
    }
}

#[async_trait]
impl RuntimeDriver for ClaudeDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Claude
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("claude") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::StreamJson,
                capabilities: CapabilitySet::MODEL_LIST | CapabilitySet::LOGIN,
            });
        }

        let auth = run_command("claude", &["auth", "status"])
            .ok()
            .and_then(|result| {
                if !result.success {
                    return Some(ProbeAuth::Unauthed);
                }
                let payload: serde_json::Value = serde_json::from_str(&result.stdout).ok()?;
                Some(if payload["loggedIn"].as_bool().unwrap_or(false) {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                })
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::StreamJson,
            capabilities: CapabilitySet::MODEL_LIST | CapabilitySet::LOGIN,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::PendingUserAction {
            message: "Run 'claude login' to authenticate".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("sonnet".into()),
            ModelInfo::from_id("opus".into()),
            ModelInfo::from_id("haiku".into()),
        ])
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let events = proc.events.clone();
        let handle = ClaudeHandle::new(key, spec, proc, HandleRole::Bootstrap);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

    async fn new_session(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        // Reuse the agent's shared process so events land on the same
        // fan-out. Each secondary handle spawns its own `claude -p` child on
        // `start()` (process-per-session); subscribers disambiguate by
        // `session_id` on the event.
        let proc = self.ensure_process(&key);
        let events = proc.events.clone();
        let handle = ClaudeHandle::new(key, spec, proc, HandleRole::Secondary);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

    async fn resume_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        session_id: SessionId,
    ) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let events = proc.events.clone();
        let mut handle = ClaudeHandle::new(key, spec, proc, HandleRole::Secondary);
        handle.preassigned_session_id = Some(session_id);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// ClaudeAgentProcess — per-agent shared state
// ---------------------------------------------------------------------------

/// Per-agent shared state. Owns the fan-out every session under this agent
/// writes into plus a live-session counter used for registry pruning.
///
/// Notably **does not own a child process or stdin channel** — those are
/// per-handle because `claude -p` cannot multiplex.
struct ClaudeAgentProcess {
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    /// Flipped by the bootstrap's `close()`. Causes `is_stale` to return true
    /// so a subsequent `attach` on the same key evicts this entry and builds
    /// a fresh [`EventFanOut`].
    closed: AtomicBool,
    /// Count of handles whose `start()` has completed and whose `close()`
    /// hasn't. Pruning on a secondary close only fires when this reaches 0.
    live_sessions: Mutex<usize>,
    /// Factory used to spawn a transport for each session's `start()`. In
    /// production points at [`spawn_real_transport`]; tests swap this for a
    /// fake factory via [`Self::set_transport_factory`].
    transport_factory: Mutex<TransportFactory>,
}

impl ClaudeAgentProcess {
    fn emit(&self, event: DriverEvent) {
        if let Err(e) = self.event_tx.try_send(event) {
            warn!(agent = %self.key, "claude v2: failed to emit event: {e}");
        }
    }

    /// True once the bootstrap's `close()` has marked this process as dead.
    /// [`ClaudeDriver::ensure_process`] evicts stale entries so the next
    /// attach builds a fresh [`EventFanOut`].
    fn is_stale(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn incr_live(&self) {
        *self.live_sessions.lock().unwrap() += 1;
    }

    /// Decrement the live-session counter. Returns the remaining count.
    fn decr_live(&self) -> usize {
        let mut guard = self.live_sessions.lock().unwrap();
        if *guard > 0 {
            *guard -= 1;
        }
        *guard
    }

    /// Install a fake transport factory on an existing shared process. Test
    /// helper — callers use this after `ensure_process` to replace the real
    /// `claude -p` spawn with an in-memory simulator.
    #[cfg(test)]
    fn set_transport_factory(&self, factory: TransportFactory) {
        *self.transport_factory.lock().unwrap() = factory;
    }

    /// Snapshot the current transport factory.
    fn transport_factory(&self) -> TransportFactory {
        Arc::clone(&self.transport_factory.lock().unwrap())
    }
}

// ---------------------------------------------------------------------------
// ClaudeHandle — AgentSessionHandle
// ---------------------------------------------------------------------------

pub struct ClaudeHandle {
    key: AgentKey,
    state: AgentState,
    spec: AgentSpec,
    /// Shared state for this agent. Provides the [`EventStreamHandle`] every
    /// session writes into.
    proc: Arc<ClaudeAgentProcess>,
    #[allow(dead_code)]
    role: HandleRole,
    /// Caller-supplied session id for the resume path. When set, `start()`
    /// passes `--resume <session_id>` to the CLI.
    preassigned_session_id: Option<SessionId>,
    /// Transport for this handle's `claude -p` child. Held in an Option so
    /// `close()` can take+drop it to trigger the Drop impl's SIGTERM.
    transport: Mutex<Option<Box<dyn ClaudeTransport>>>,
    stdin_tx: Option<mpsc::Sender<String>>,
    shared: Option<Arc<Mutex<SharedReaderState>>>,
    /// Session id cache synced from the stdout reader. The reader writes
    /// here on `system.init` so `session_id()` can return `Option<&str>`
    /// without needing to lock the shared state (and without the lifetime
    /// gymnastics of borrowing across a mutex guard).
    ///
    /// `OnceLock` because Claude's session id is established exactly once
    /// per handle — the resumed and newly-minted paths both land on the same
    /// `system.init` emission. Shared with the reader task via `Arc`.
    session_id: Arc<OnceLock<String>>,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
    /// `true` once `start()` has successfully spun up the child and we've
    /// bumped the proc's live-session counter. Guards against double
    /// decrement in `close()`.
    started: bool,
}

/// Mutable state shared between the handle and the stdout reader task.
struct SharedReaderState {
    session_id: Option<String>,
    run_id: Option<RunId>,
    agent_state: AgentState,
}

impl ClaudeHandle {
    fn new(
        key: AgentKey,
        spec: AgentSpec,
        proc: Arc<ClaudeAgentProcess>,
        role: HandleRole,
    ) -> Self {
        Self {
            key,
            state: AgentState::Idle,
            spec,
            proc,
            role,
            preassigned_session_id: None,
            transport: Mutex::new(None),
            stdin_tx: None,
            shared: None,
            session_id: Arc::new(OnceLock::new()),
            reader_handles: Vec::new(),
            started: false,
        }
    }

    fn emit(&self, event: DriverEvent) {
        self.proc.emit(event);
    }
}

#[async_trait]
impl AgentSessionHandle for ClaudeHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn session_id(&self) -> Option<&str> {
        // The stdout reader writes the minted session id into `self.session_id`
        // on `system.init` (see `spawn_stdout_reader`). `self.state` is NOT a
        // reliable source here — it's advanced to `Starting`/`PromptInFlight`/
        // `Closed` from the handle's own methods but never to `Active`, because
        // the transition to `Active` lives in the reader task under
        // `shared.agent_state`. Reading from the `OnceLock` lets us return a
        // borrow without the lifetime-vs-mutex-guard tangle, and `OnceLock`
        // matches the semantics: Claude's session id is assigned exactly once
        // per handle (new or resumed — both land on `system.init`).
        //
        // Fall back to the pre-assigned id for the resume path when callers
        // inspect the handle before `start()` runs and the reader has produced
        // its first `system.init` line.
        self.session_id
            .get()
            .map(String::as_str)
            .or(self.preassigned_session_id.as_deref())
    }

    fn state(&self) -> AgentState {
        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state.clone()
        } else {
            self.state.clone()
        }
    }

    async fn start(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
        if !matches!(self.state, AgentState::Idle) {
            bail!("claude v2: start called in non-idle state");
        }

        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Write MCP config file
        let wd = &self.spec.working_directory;
        let mcp_config_path = wd.join(".chorus-claude-mcp.json");
        let token = super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
            .await
            .context("failed to pair with shared bridge")?;
        let mcp_config = build_mcp_config(&self.spec.bridge_endpoint, &token);
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .context("failed to write MCP config")?;

        // Determine which session id (if any) to resume. Prefer the
        // handle-level preassigned id over the per-call opts, but support
        // both entry points for symmetry with the other drivers.
        let resume_id = self
            .preassigned_session_id
            .clone()
            .or(opts.resume_session_id);

        // Build CLI args
        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--permission-mode".into(),
            "acceptEdits".into(),
            "--allowedTools".into(),
            "Bash,Read,Edit,Write,MultiEdit,Glob,Grep,LS,mcp__chat__*".into(),
            "--mcp-config".into(),
            mcp_path_str,
        ];
        if !self.spec.model.is_empty() {
            args.push("--model".into());
            args.push(self.spec.model.clone());
        }
        if let Some(ref prompt) = self.spec.system_prompt {
            args.push("--append-system-prompt".into());
            args.push(prompt.clone());
        }
        if let Some(ref sid) = resume_id {
            args.push("--resume".into());
            args.push(sid.clone());
        }

        let factory = self.proc.transport_factory();
        let mut transport = factory(args, &self.spec)?;

        let stdout = transport.take_stdout();
        let maybe_stderr = transport.take_stderr();
        let stdin_writer = transport.take_stdin();

        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);
        self.reader_handles
            .push(spawn_stdin_writer(stdin_writer, stdin_rx));

        // Claude CLI in stream-json mode requires stdin input before emitting
        // the system/init event. Send the initial prompt immediately so the
        // init event (and subsequent stream events) flow without deadlock.
        let initial_run_id = if let Some(ref req) = init_prompt {
            let rid = RunId::new_v4();
            let msg = claude_headless::build_user_message(&req.text);
            let _ = stdin_tx.send(format!("{msg}\n")).await;
            Some(rid)
        } else {
            None
        };

        let shared = Arc::new(Mutex::new(SharedReaderState {
            session_id: None,
            run_id: initial_run_id,
            agent_state: AgentState::Starting,
        }));
        self.shared = Some(shared.clone());

        self.reader_handles.push(spawn_stdout_reader(
            self.key.clone(),
            self.proc.event_tx.clone(),
            stdout,
            shared,
            Arc::clone(&self.session_id),
        ));

        if let Some(stderr) = maybe_stderr {
            self.reader_handles.push(spawn_stderr_reader(stderr));
        }

        *self.transport.lock().unwrap() = Some(transport);
        self.stdin_tx = Some(stdin_tx);

        self.proc.incr_live();
        self.started = true;

        Ok(())
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = if let Some(ref shared) = self.shared {
            let guard = shared.lock().unwrap();
            match &guard.agent_state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("claude v2: prompt called in non-active state"),
            }
        } else {
            match &self.state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("claude v2: prompt called in non-active state"),
            }
        };

        let run_id = RunId::new_v4();

        let msg = claude_headless::build_user_message(&req.text);

        let stdin_tx = self
            .stdin_tx
            .as_ref()
            .context("claude v2: stdin not available — handle not started")?;
        stdin_tx
            .send(format!("{msg}\n"))
            .await
            .map_err(|e| anyhow!("claude v2: stdin write failed: {e}"))?;

        if let Some(ref shared) = self.shared {
            let mut guard = shared.lock().unwrap();
            guard.run_id = Some(run_id);
            guard.agent_state = AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }
        self.state = AgentState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::PromptInFlight { run_id, session_id },
        });

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        // Claude v2 headless does not support in-flight cancellation; the
        // session must be closed and restarted. Return NotInFlight so the
        // caller receives a no-op rather than invalid session data.
        Ok(CancelOutcome::NotInFlight)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, AgentState::Closed) {
            return Ok(());
        }

        // Terminate this handle's own child. Unlike kimi/opencode, claude
        // spawns one `claude -p` per session (the CLI can't multiplex), so
        // this is strictly per-handle — no live sibling depends on it.
        if let Some(mut transport) = self.transport.lock().unwrap().take() {
            transport.terminate();
        }

        for handle in self.reader_handles.drain(..) {
            handle.abort();
        }

        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state = AgentState::Closed;
        }
        self.state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });

        // Live-session accounting: only decrement if start() actually ran
        // (otherwise we'd underflow the counter for attach→close without
        // start).
        let remaining = if self.started {
            self.started = false;
            self.proc.decr_live()
        } else {
            *self.proc.live_sessions.lock().unwrap()
        };

        // Shared-process teardown (fan-out drain + registry prune) is gated
        // on `remaining == 0`, regardless of role. The bootstrap previously
        // tore these down unconditionally even when secondaries were still
        // mid-prompt — that closed the shared fan-out and removed the
        // registry entry while live secondaries were still emitting events
        // into both. The last session to close (either role) triggers
        // teardown here; a bootstrap close with a live secondary just
        // quiesces this handle's per-session child.
        if remaining == 0 {
            self.proc.closed.store(true, Ordering::SeqCst);
            self.proc.events.close();
            agent_instances().lock().unwrap().remove(&self.key);
        }

        Ok(())
    }
}

impl Drop for ClaudeHandle {
    fn drop(&mut self) {
        // Belt-and-suspenders: if `close()` wasn't called, signal the child
        // SIGTERM on drop so we don't leak `claude -p` processes.
        if let Ok(mut guard) = self.transport.lock() {
            if let Some(ref mut transport) = *guard {
                transport.terminate();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

fn spawn_stdout_reader<R: AsyncRead + Unpin + Send + 'static>(
    key: AgentKey,
    tx: mpsc::Sender<DriverEvent>,
    stdout: R,
    shared: Arc<Mutex<SharedReaderState>>,
    handle_session_id: Arc<OnceLock<String>>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        // Tool call accumulator: (tool_id, tool_name, json_buffer, start_index)
        let mut pending_tool: Option<(String, String, String, u32)> = None;

        while let Ok(Some(line)) = lines.next_line().await {
            let parsed = claude_headless::parse_line(&line);
            trace!(key = %key, ?parsed, "claude headless line");

            match parsed {
                HeadlessEvent::SystemInit { session_id } => {
                    debug!(key = %key, session_id = %session_id, "claude headless: system init");

                    // Publish the session id to the handle-level cache so
                    // `session_id()` can return a borrow without touching the
                    // shared mutex. Ignored on Err: `set` fails only if the
                    // cell is already populated, which happens when a resumed
                    // child re-emits its same id — no-op is correct.
                    let _ = handle_session_id.set(session_id.clone());

                    // Capture whether there's already a run in flight (initial
                    // prompt was sent on stdin before init arrived).
                    let pre_existing_run_id = {
                        let mut guard = shared.lock().unwrap();
                        guard.session_id = Some(session_id.clone());
                        guard.agent_state = AgentState::Active {
                            session_id: session_id.clone(),
                        };
                        guard.run_id
                    };

                    let _ = tx
                        .send(DriverEvent::SessionAttached {
                            key: key.clone(),
                            session_id: session_id.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(DriverEvent::Lifecycle {
                            key: key.clone(),
                            state: AgentState::Active {
                                session_id: session_id.clone(),
                            },
                        })
                        .await;

                    // If a prompt was already sent before init, emit
                    // PromptInFlight so the manager knows we're working.
                    if let Some(run_id) = pre_existing_run_id {
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.agent_state = AgentState::PromptInFlight {
                                run_id,
                                session_id: session_id.clone(),
                            };
                        }
                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::PromptInFlight {
                                    run_id,
                                    session_id: session_id.clone(),
                                },
                            })
                            .await;
                    }
                }

                HeadlessEvent::ApiRetry { attempt, error } => {
                    trace!(key = %key, attempt, %error, "claude headless: api retry");
                }

                HeadlessEvent::ThinkingDelta { text } => {
                    let (run_id, session_id) = {
                        let g = shared.lock().unwrap();
                        (g.run_id, g.session_id.clone().unwrap_or_default())
                    };
                    if let Some(rid) = run_id {
                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                session_id,
                                run_id: rid,
                                item: AgentEventItem::Thinking { text },
                            })
                            .await;
                    }
                }

                HeadlessEvent::TextDelta { text } => {
                    let (run_id, session_id) = {
                        let g = shared.lock().unwrap();
                        (g.run_id, g.session_id.clone().unwrap_or_default())
                    };
                    if let Some(rid) = run_id {
                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                session_id,
                                run_id: rid,
                                item: AgentEventItem::Text { text },
                            })
                            .await;
                    }
                }

                HeadlessEvent::ToolUseStart { index, id, name } => {
                    // Flush any pending tool call
                    let (run_id, session_id) = {
                        let g = shared.lock().unwrap();
                        (g.run_id, g.session_id.clone().unwrap_or_default())
                    };
                    if let Some(rid) = run_id {
                        if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                            let input: serde_json::Value =
                                serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    session_id,
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall { name: tname, input },
                                })
                                .await;
                        }
                    }
                    // Start new accumulation
                    pending_tool = Some((id, name, String::new(), index));
                }

                HeadlessEvent::InputJsonDelta { partial_json, .. } => {
                    if let Some(ref mut tool) = pending_tool {
                        tool.2.push_str(&partial_json);
                    }
                }

                HeadlessEvent::ContentBlockStop { index }
                | HeadlessEvent::ToolUseStop { index } => {
                    // If index matches current tool accumulation, flush it
                    let should_flush = pending_tool.as_ref().is_some_and(|t| t.3 == index);
                    if should_flush {
                        let (run_id, session_id) = {
                            let g = shared.lock().unwrap();
                            (g.run_id, g.session_id.clone().unwrap_or_default())
                        };
                        if let Some(rid) = run_id {
                            if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                                let input: serde_json::Value =
                                    serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                                let _ = tx
                                    .send(DriverEvent::Output {
                                        key: key.clone(),
                                        session_id,
                                        run_id: rid,
                                        item: AgentEventItem::ToolCall { name: tname, input },
                                    })
                                    .await;
                            }
                        }
                    }
                }

                HeadlessEvent::TurnResult { session_id, .. } => {
                    let run_id = shared.lock().unwrap().run_id;
                    if let Some(rid) = run_id {
                        let resolved_sid = if session_id.is_empty() {
                            shared
                                .lock()
                                .unwrap()
                                .session_id
                                .clone()
                                .unwrap_or_default()
                        } else {
                            session_id
                        };

                        // Flush remaining tool calls
                        if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                            let input: serde_json::Value =
                                serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    session_id: resolved_sid.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall { name: tname, input },
                                })
                                .await;
                        }

                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: resolved_sid.clone(),
                                run_id: rid,
                                item: AgentEventItem::TurnEnd,
                            })
                            .await;
                        let _ = tx
                            .send(DriverEvent::Completed {
                                key: key.clone(),
                                session_id: resolved_sid.clone(),
                                run_id: rid,
                                result: RunResult {
                                    finish_reason: FinishReason::Natural,
                                },
                            })
                            .await;
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.run_id = None;
                            guard.agent_state = AgentState::Active {
                                session_id: resolved_sid.clone(),
                            };
                        }

                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active {
                                    session_id: resolved_sid,
                                },
                            })
                            .await;
                    }
                }

                HeadlessEvent::Unknown => {}
            }
        }

        // stdout EOF
        {
            let mut guard = shared.lock().unwrap();
            guard.agent_state = AgentState::Closed;
        }
        let _ = tx
            .send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            })
            .await;
    })
}

fn spawn_stderr_reader<R: AsyncRead + Unpin + Send + 'static>(
    stderr: R,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!(target: "claude_v2_stderr", "{}", line);
        }
    })
}

fn spawn_stdin_writer(
    mut stdin: Box<dyn AsyncWrite + Send + Unpin>,
    mut rx: mpsc::Receiver<String>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::AsyncWriteExt;

    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(data.as_bytes()).await {
                warn!("claude v2: stdin write failed: {e}");
                break;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::io::{duplex, AsyncWriteExt, DuplexStream};
    use tokio::sync::Mutex as TokioMutex;
    use tokio::time::timeout;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-claude".into(),
            description: None,
            system_prompt: None,
            model: "sonnet".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".into(),
        }
    }

    #[tokio::test]
    async fn test_claude_driver_probe_not_installed() {
        // claude binary is not on PATH in CI/test environments
        let driver = ClaudeDriver;
        let probe = driver.probe().await.unwrap();
        // Either NotInstalled or Unauthed depending on host — both are valid.
        // The key invariant: transport and capabilities are always set.
        assert_eq!(probe.transport, TransportKind::StreamJson);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        assert!(probe.capabilities.contains(CapabilitySet::LOGIN));
    }

    #[tokio::test]
    async fn test_claude_driver_list_models() {
        let driver = ClaudeDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "sonnet");
        assert_eq!(models[1].id, "opus");
        assert_eq!(models[2].id, "haiku");
    }

    #[tokio::test]
    async fn test_claude_driver_attach_returns_idle() {
        let driver = ClaudeDriver;
        // Ensure no leftover registry entry from another test with the same key.
        agent_instances()
            .lock()
            .unwrap()
            .remove("agent-claude-attach-returns-idle");
        let result = driver
            .attach("agent-claude-attach-returns-idle".into(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    // ---- build_mcp_config tests ----

    #[test]
    fn build_mcp_config_http_shape() {
        // Native HTTP MCP shape — the only shape we produce.
        let config = build_mcp_config("http://127.0.0.1:4321", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["type"], "http");
        assert_eq!(chat["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert!(chat.get("command").is_none());
        assert!(chat.get("args").is_none());
    }

    #[test]
    fn build_mcp_config_trims_trailing_slash() {
        // Endpoint with trailing slash must not produce `//token/` in the URL.
        let config = build_mcp_config("http://127.0.0.1:4321/", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }

    /// Feed captured JSONL through spawn_stdout_reader via a mock pipe and
    /// verify the DriverEvent sequence matches expectations.
    #[tokio::test]
    async fn test_stdout_reader_full_turn() {
        // Captured JSONL: system init → thinking → text → tool_use → result
        let jsonl = [
            r#"{"type":"system","subtype":"init","session_id":"sess-test","tools":["Bash"],"mcp_servers":[],"model":"claude-sonnet-4-6"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"Read","input":{}}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"file\":"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"main.rs\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":2}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hi","stop_reason":"end_turn","session_id":"sess-test","duration_ms":500}"#,
        ];

        // Create a duplex stream as mock stdout
        let (mock_stdout, mut writer) = duplex(4096);

        // Set up shared state and channels
        let (event_tx, mut event_rx) = mpsc::channel::<DriverEvent>(64);
        let initial_run_id = RunId::new_v4();
        let shared = Arc::new(Mutex::new(SharedReaderState {
            session_id: None,
            run_id: Some(initial_run_id),
            agent_state: AgentState::Starting,
        }));

        // Spawn the reader
        let _handle = spawn_stdout_reader(
            "test-agent".into(),
            event_tx,
            mock_stdout,
            shared.clone(),
            Arc::new(OnceLock::new()),
        );

        // Write JSONL lines
        for line in &jsonl {
            writer.write_all(line.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
        // Close writer to trigger EOF
        drop(writer);

        // Collect events (with timeout)
        let mut events = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_secs(2), event_rx.recv()).await {
            events.push(ev);
        }

        // Verify event sequence
        // 1. SessionAttached
        assert!(
            matches!(&events[0], DriverEvent::SessionAttached { session_id, .. } if session_id == "sess-test"),
            "expected SessionAttached, got {:?}",
            events[0]
        );
        // 2. Lifecycle(Active)
        assert!(
            matches!(&events[1], DriverEvent::Lifecycle { state: AgentState::Active { session_id }, .. } if session_id == "sess-test"),
            "expected Lifecycle(Active), got {:?}",
            events[1]
        );
        // 3. Lifecycle(PromptInFlight) — from deferred prompt
        assert!(
            matches!(
                &events[2],
                DriverEvent::Lifecycle {
                    state: AgentState::PromptInFlight { .. },
                    ..
                }
            ),
            "expected Lifecycle(PromptInFlight), got {:?}",
            events[2]
        );
        // 4. Thinking delta
        assert!(
            matches!(&events[3], DriverEvent::Output { item: AgentEventItem::Thinking { text }, .. } if text == "hmm"),
            "expected Thinking, got {:?}",
            events[3]
        );
        // 5. Text delta
        assert!(
            matches!(&events[4], DriverEvent::Output { item: AgentEventItem::Text { text }, .. } if text == "Hi"),
            "expected Text, got {:?}",
            events[4]
        );
        // 6. ToolCall (flushed on content_block_stop)
        match &events[5] {
            DriverEvent::Output {
                item: AgentEventItem::ToolCall { name, input },
                ..
            } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file"], "main.rs");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // 7. TurnEnd
        assert!(
            matches!(
                &events[6],
                DriverEvent::Output {
                    item: AgentEventItem::TurnEnd,
                    ..
                }
            ),
            "expected TurnEnd, got {:?}",
            events[6]
        );
        // 8. Completed
        assert!(
            matches!(
                &events[7],
                DriverEvent::Completed {
                    result: RunResult {
                        finish_reason: FinishReason::Natural,
                        ..
                    },
                    ..
                }
            ),
            "expected Completed, got {:?}",
            events[7]
        );
        // 9. Lifecycle(Active) — reset after completion
        assert!(
            matches!(
                &events[8],
                DriverEvent::Lifecycle {
                    state: AgentState::Active { .. },
                    ..
                }
            ),
            "expected Lifecycle(Active), got {:?}",
            events[8]
        );
        // 10. Lifecycle(Closed) — EOF
        assert!(
            matches!(
                &events[9],
                DriverEvent::Lifecycle {
                    state: AgentState::Closed,
                    ..
                }
            ),
            "expected Lifecycle(Closed), got {:?}",
            events[9]
        );
        assert_eq!(events.len(), 10);

        // Verify shared state ended as Closed
        let final_state = shared.lock().unwrap().agent_state.clone();
        assert!(matches!(final_state, AgentState::Closed));
    }

    // -----------------------------------------------------------------------
    // Fake transport for multi-session tests
    // -----------------------------------------------------------------------

    /// In-memory transport: a handle's stdin writes route to a DuplexStream
    /// that the test's "simulator" task reads; the simulator writes
    /// canned stdout back on another DuplexStream the handle reads. No real
    /// `claude` binary required.
    struct FakeClaudeTransport {
        stdout_reader: Option<Box<dyn AsyncRead + Send + Unpin>>,
        stdin_writer: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    }

    impl ClaudeTransport for FakeClaudeTransport {
        fn take_stdout(&mut self) -> Box<dyn AsyncRead + Send + Unpin> {
            self.stdout_reader.take().expect("stdout taken twice")
        }
        fn take_stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
            None
        }
        fn take_stdin(&mut self) -> Box<dyn AsyncWrite + Send + Unpin> {
            self.stdin_writer.take().expect("stdin taken twice")
        }
        fn terminate(&mut self) {}
    }

    /// One recorded fake-child spawn: the args the factory received plus an
    /// id we can compare for equality (distinct per spawn).
    #[derive(Clone)]
    struct SpawnedRecord {
        args: Vec<String>,
        instance_id: usize,
    }

    /// Shared state captured by a fake transport factory. Each `start()`
    /// appends a `SpawnedRecord` and wires up a simulator writer handle the
    /// test can use to inject lines into that specific child's stdout.
    #[derive(Default)]
    struct FakeFactoryState {
        spawns: Vec<SpawnedRecord>,
        /// Writer halves keyed by instance_id so tests can target a specific
        /// child's stdout for injected lines.
        stdout_writers: HashMap<usize, Arc<TokioMutex<DuplexStream>>>,
        next_id: usize,
    }

    fn install_fake_factory(proc: &Arc<ClaudeAgentProcess>) -> Arc<Mutex<FakeFactoryState>> {
        // Short-circuit the pairing HTTP call: we never hit the real bridge
        // because the test factory is invoked AFTER `request_pairing_token`
        // in `start()`. The tests that use this factory bypass the bridge
        // call by disabling pairing — see `start_with_fake` helper below.
        let state = Arc::new(Mutex::new(FakeFactoryState::default()));
        let state_cl = Arc::clone(&state);
        let factory: TransportFactory = Arc::new(move |args, _spec| {
            // Build a pair of duplex streams:
            //  - stdout: simulator writes, handle reads
            //  - stdin : handle writes, simulator reads (discarded here)
            let (stdout_writer, stdout_reader): (DuplexStream, DuplexStream) = duplex(64 * 1024);
            let (stdin_writer, _stdin_reader): (DuplexStream, DuplexStream) = duplex(64 * 1024);

            let transport = FakeClaudeTransport {
                stdout_reader: Some(Box::new(stdout_reader)),
                stdin_writer: Some(Box::new(stdin_writer)),
            };

            let mut guard = state_cl.lock().unwrap();
            let instance_id = guard.next_id;
            guard.next_id += 1;
            guard.spawns.push(SpawnedRecord {
                args: args.clone(),
                instance_id,
            });
            guard
                .stdout_writers
                .insert(instance_id, Arc::new(TokioMutex::new(stdout_writer)));

            Ok(Box::new(transport) as Box<dyn ClaudeTransport>)
        });
        proc.set_transport_factory(factory);
        state
    }

    /// Spawn a mock bridge that always returns `{"token": "tok-test"}` so
    /// `request_pairing_token` in `start()` succeeds without a live runtime.
    async fn spawn_mock_bridge() -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::post;
        use axum::Router;

        let app = Router::new().route(
            "/admin/pair",
            post(|| async {
                axum::Json(serde_json::json!({"token": "tok-test"}))
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(25)).await;
        (url, handle)
    }

    fn test_spec_with_bridge(wd: &std::path::Path, bridge: &str) -> AgentSpec {
        AgentSpec {
            display_name: "test-claude".into(),
            description: None,
            system_prompt: None,
            model: "sonnet".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: wd.to_path_buf(),
            bridge_endpoint: bridge.into(),
        }
    }

    /// Helper: write a `system.init` line to the fake child's stdout so
    /// `spawn_stdout_reader` transitions to Active and emits
    /// `SessionAttached`.
    async fn feed_system_init(
        factory: &Arc<Mutex<FakeFactoryState>>,
        instance_id: usize,
        session_id: &str,
    ) {
        let writer = factory
            .lock()
            .unwrap()
            .stdout_writers
            .get(&instance_id)
            .expect("instance exists")
            .clone();
        let line = format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session_id}","tools":[],"mcp_servers":[],"model":"claude-sonnet"}}"#
        );
        let mut w = writer.lock().await;
        w.write_all(line.as_bytes()).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        w.flush().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Multi-session tests
    // -----------------------------------------------------------------------

    /// Test: `attach` + `new_session` wire both handles to the same shared
    /// `EventStreamHandle` so every session's events land on one timeline.
    #[tokio::test]
    async fn attach_and_new_session_share_event_stream() {
        let driver = ClaudeDriver;
        let key = "agent-claude-share-stream".to_string();
        // Scrub any leftover entry.
        agent_instances().lock().unwrap().remove(&key);

        let a1 = driver.attach(key.clone(), test_spec()).await.unwrap();
        let a2 = driver
            .new_session(key.clone(), test_spec())
            .await
            .unwrap();

        // Both `EventStreamHandle`s point at the same `Arc<EventFanOut>`.
        assert!(
            Arc::ptr_eq(&a1.events.inner, &a2.events.inner),
            "attach and new_session must share the same EventFanOut Arc"
        );

        // Clean up registry for subsequent tests.
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Test: each session spawns its own `claude -p` child. The fake factory
    /// records one `SpawnedRecord` per `start()`; two distinct starts yield
    /// two distinct records with different `instance_id`s.
    #[tokio::test]
    async fn new_session_spawns_a_distinct_child() {
        let (bridge_url, _bridge) = spawn_mock_bridge().await;
        let tmp = tempfile::tempdir().unwrap();

        let driver = ClaudeDriver;
        let key = "agent-claude-distinct-children".to_string();
        agent_instances().lock().unwrap().remove(&key);

        let spec = test_spec_with_bridge(tmp.path(), &bridge_url);

        let attach = driver.attach(key.clone(), spec.clone()).await.unwrap();
        let factory = install_fake_factory(&driver.ensure_process(&key));

        let new_sess = driver
            .new_session(key.clone(), spec.clone())
            .await
            .unwrap();

        let mut h1 = attach.handle;
        let mut h2 = new_sess.handle;

        h1.start(StartOpts::default(), None).await.unwrap();
        h2.start(StartOpts::default(), None).await.unwrap();

        {
            let state = factory.lock().unwrap();
            assert_eq!(
                state.spawns.len(),
                2,
                "two start() calls must spawn two children"
            );
            assert_ne!(
                state.spawns[0].instance_id, state.spawns[1].instance_id,
                "each child must have a distinct instance id"
            );
        }

        // Tear down.
        h1.close().await.unwrap();
        h2.close().await.unwrap();
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Test: `resume_session("sess_xyz")` + `start` passes `--resume
    /// sess_xyz` to the spawned child's command line.
    #[tokio::test]
    async fn resume_session_passes_resume_flag() {
        let (bridge_url, _bridge) = spawn_mock_bridge().await;
        let tmp = tempfile::tempdir().unwrap();

        let driver = ClaudeDriver;
        let key = "agent-claude-resume-flag".to_string();
        agent_instances().lock().unwrap().remove(&key);

        let spec = test_spec_with_bridge(tmp.path(), &bridge_url);

        // Bring the agent online first with an attach so the registry has an
        // entry we can install the fake factory on.
        let attach = driver.attach(key.clone(), spec.clone()).await.unwrap();
        let factory = install_fake_factory(&driver.ensure_process(&key));

        let resumed = driver
            .resume_session(key.clone(), spec.clone(), "sess_xyz".to_string())
            .await
            .unwrap();

        let mut hr = resumed.handle;
        hr.start(StartOpts::default(), None).await.unwrap();

        // Find the --resume flag in the captured spawn args.
        {
            let state = factory.lock().unwrap();
            assert_eq!(state.spawns.len(), 1);
            let args = &state.spawns[0].args;
            let mut found = false;
            for w in args.windows(2) {
                if w[0] == "--resume" && w[1] == "sess_xyz" {
                    found = true;
                    break;
                }
            }
            assert!(
                found,
                "expected --resume sess_xyz in spawn args, got: {args:?}"
            );
        }

        hr.close().await.unwrap();
        // Also close the bootstrap to clean up the registry.
        let mut ha = attach.handle;
        ha.close().await.unwrap();
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Regression test: attach → close (bootstrap) → re-attach on same key
    /// must build a fresh `ClaudeAgentProcess` with a fresh
    /// `EventStreamHandle`, not recycle the torn-down Arc.
    #[tokio::test]
    async fn attach_close_reattach_spawns_fresh_process() {
        let driver = ClaudeDriver;
        let key = "agent-claude-reattach".to_string();
        agent_instances().lock().unwrap().remove(&key);

        // --- round 1 ---
        let a1 = driver.attach(key.clone(), test_spec()).await.unwrap();
        let proc_v1_addr = Arc::as_ptr(&driver.ensure_process(&key)) as usize;
        let events_v1 = a1.events.clone();
        let mut h1 = a1.handle;
        h1.close().await.unwrap();

        // Registry entry must be gone so re-attach builds a fresh proc.
        assert!(
            agent_instances().lock().unwrap().get(&key).is_none(),
            "bootstrap close must prune the registry"
        );

        // --- round 2 ---
        let a2 = driver.attach(key.clone(), test_spec()).await.unwrap();
        let proc_v2_addr = Arc::as_ptr(&driver.ensure_process(&key)) as usize;
        assert_ne!(
            proc_v1_addr, proc_v2_addr,
            "re-attach must build a fresh ClaudeAgentProcess"
        );
        assert!(
            !Arc::ptr_eq(&events_v1.inner, &a2.events.inner),
            "re-attach must build a fresh EventFanOut"
        );

        // Clean up.
        let mut h2 = a2.handle;
        h2.close().await.unwrap();
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Test: starting a child and feeding it a SystemInit with session_id
    /// `sess_abc` on stdout produces a `DriverEvent::SessionAttached {
    /// session_id: "sess_abc" }` on the shared EventStreamHandle.
    #[tokio::test]
    async fn session_attached_event_carries_session_id() {
        let (bridge_url, _bridge) = spawn_mock_bridge().await;
        let tmp = tempfile::tempdir().unwrap();

        let driver = ClaudeDriver;
        let key = "agent-claude-session-attached".to_string();
        agent_instances().lock().unwrap().remove(&key);

        let spec = test_spec_with_bridge(tmp.path(), &bridge_url);
        let attach = driver.attach(key.clone(), spec.clone()).await.unwrap();
        let factory = install_fake_factory(&driver.ensure_process(&key));

        let mut sub = attach.events.subscribe();

        let mut h = attach.handle;
        h.start(StartOpts::default(), None).await.unwrap();

        // Instance id 0 is the first spawn. Inject system.init.
        feed_system_init(&factory, 0, "sess_abc").await;

        // First couple of events should include a SessionAttached with our sid.
        let mut found = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
                Ok(Some(DriverEvent::SessionAttached { session_id, .. }))
                    if session_id == "sess_abc" =>
                {
                    found = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        assert!(
            found,
            "expected DriverEvent::SessionAttached{{ session_id: sess_abc }} on the shared stream"
        );

        h.close().await.unwrap();
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Regression guard for the Stage 2 bug where `ClaudeHandle::session_id()`
    /// read only from `self.state` — which is never advanced to `Active`,
    /// because the `Active` transition lives in the stdout reader task and
    /// writes to `shared.agent_state`. The fix wires the reader's
    /// `system.init` branch through an `OnceLock<String>` the handle owns, so
    /// `session_id()` observes the minted id without touching the shared
    /// mutex. Pre-fix this assertion failed (returned `None`); post-fix it
    /// returns `Some("sess_zzz")`. The live integration test
    /// `claude_multi_session_bootstrap_close_preserves_secondary` caught
    /// this, but the fake-transport unit tests missed it — hence this
    /// targeted test.
    #[tokio::test]
    async fn session_id_returns_value_after_system_init() {
        let (bridge_url, _bridge) = spawn_mock_bridge().await;
        let tmp = tempfile::tempdir().unwrap();

        let driver = ClaudeDriver;
        let key = format!("agent-claude-sid-after-init-{}", uuid::Uuid::new_v4());
        agent_instances().lock().unwrap().remove(&key);

        let spec = test_spec_with_bridge(tmp.path(), &bridge_url);
        let attach = driver.attach(key.clone(), spec.clone()).await.unwrap();
        let factory = install_fake_factory(&driver.ensure_process(&key));

        let mut h = attach.handle;

        // Before start(): no id to report.
        assert_eq!(
            h.session_id(),
            None,
            "session_id() must be None before start() runs the reader"
        );

        h.start(StartOpts::default(), None).await.unwrap();

        // Inject system.init for instance 0 — the reader publishes the id
        // into the handle's OnceLock cache.
        feed_system_init(&factory, 0, "sess_zzz").await;

        // The reader task is async; give it a brief window to process the
        // line and set the cache. Polling — not fixed sleep — so the test
        // stays fast when the reader is already done.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut seen: Option<String> = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(sid) = h.session_id() {
                seen = Some(sid.to_string());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            seen.as_deref(),
            Some("sess_zzz"),
            "session_id() must reflect the id emitted on system.init"
        );

        h.close().await.unwrap();
        agent_instances().lock().unwrap().remove(&key);
    }

    /// Regression for the Stage 2 ship-blocker: closing the bootstrap handle
    /// while a secondary session is still live (mid-prompt) must NOT close
    /// the shared fan-out or prune the registry entry. Claude's per-session
    /// child is already private — what bootstrap close tore down
    /// unconditionally was the *shared* state (`proc.closed`, the fan-out,
    /// and the `agent_instances` registry entry). With the fix, that
    /// shared-state teardown is gated on `live_sessions == 0` regardless
    /// of role.
    #[tokio::test]
    async fn bootstrap_close_with_live_secondary_does_not_tear_down_shared_child() {
        let (bridge_url, _bridge) = spawn_mock_bridge().await;
        let tmp = tempfile::tempdir().unwrap();

        let driver = ClaudeDriver;
        let key = format!("claude-bootstrap-live-secondary-{}", uuid::Uuid::new_v4());
        agent_instances().lock().unwrap().remove(&key);

        let spec = test_spec_with_bridge(tmp.path(), &bridge_url);

        // Bring the shared process online via the driver and install a fake
        // transport factory so `start()` doesn't try to spawn real `claude -p`.
        let attach = driver.attach(key.clone(), spec.clone()).await.unwrap();
        let _factory = install_fake_factory(&driver.ensure_process(&key));
        let proc = driver.ensure_process(&key);
        let events_handle = attach.events.clone();

        let new_sess = driver.new_session(key.clone(), spec.clone()).await.unwrap();

        let mut bootstrap_handle = attach.handle;
        let mut secondary_handle = new_sess.handle;

        // Start both handles — each spawns its own (fake) `claude -p` child
        // and bumps `live_sessions`. After this, live_sessions == 2.
        bootstrap_handle.start(StartOpts::default(), None).await.unwrap();
        secondary_handle.start(StartOpts::default(), None).await.unwrap();

        assert_eq!(
            *proc.live_sessions.lock().unwrap(),
            2,
            "start() on both handles should bring live_sessions to 2"
        );

        // Force secondary into PromptInFlight to model the race we're
        // defending against: bootstrap close landing while the secondary
        // is mid-stream. We don't have a real child piping events, so we
        // manipulate the shared state directly — state() reads from here.
        // (We down-cast via the concrete type via proc.live_sessions snapshot
        // — the invariant we care about is shared-state teardown gating.)
        //
        // Note: we can't mutate secondary_handle.shared directly because
        // AgentSessionHandle is a trait object. We rely on live_sessions
        // instead, which is the actual teardown gate for this driver.

        // ---- Close the bootstrap while the secondary is still started. ----
        bootstrap_handle.close().await.unwrap();

        assert_eq!(
            *proc.live_sessions.lock().unwrap(),
            1,
            "bootstrap close should decrement live_sessions to 1"
        );
        assert!(
            !proc.closed.load(Ordering::SeqCst),
            "bootstrap close with a live secondary must NOT set proc.closed"
        );
        assert!(
            !events_handle.inner.closing.load(Ordering::SeqCst),
            "bootstrap close with a live secondary must NOT close the fan-out"
        );
        assert!(
            agent_instances().lock().unwrap().get(&key).is_some(),
            "bootstrap close with a live secondary must NOT prune the registry entry"
        );

        // ---- Close the secondary. Now teardown fires. ----
        secondary_handle.close().await.unwrap();

        assert_eq!(
            *proc.live_sessions.lock().unwrap(),
            0,
            "last-session close should decrement live_sessions to 0"
        );
        assert!(
            proc.closed.load(Ordering::SeqCst),
            "last-session close must set proc.closed so a re-attach rebuilds"
        );
        assert!(
            events_handle.inner.closing.load(Ordering::SeqCst),
            "last-session close must signal the fan-out to drain"
        );
        assert!(
            agent_instances().lock().unwrap().get(&key).is_none(),
            "last-session close must prune the registry entry"
        );
    }
}
