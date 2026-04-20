//! v2 Codex driver backed by the `codex app-server` native protocol.
//!
//! Uses JSONL over stdio with the Codex app-server wire format, which omits
//! the `"jsonrpc":"2.0"` header present in ACP messages. See the companion
//! module [`super::codex_app_server`] for all message builders and parsers.
//!
//! # Multi-session
//!
//! One `codex app-server` child process hosts any number of threads
//! (sessions) via `thread/start` and `thread/resume`. The driver keeps a
//! per-agent [`CodexAgentProcess`] in [`CodexDriver::agent_instances`];
//! `attach`, `new_session`, and `resume_session` all look up or create the
//! same process and hand back a fresh [`CodexHandle`] that writes into the
//! shared stdin and reads from the shared event stream. Each handle owns
//! only its own session-scoped state (`thread_id`, in-flight `run_id`).
//!
//! Response routing is id-agnostic (via
//! [`super::codex_app_server::parse_line_with_registry`]): every request
//! allocates a fresh numeric id recorded in
//! [`SharedReaderState::pending_requests`]; the stdout reader task looks
//! the id up to classify the reply. Notifications (`turn/started`,
//! `item/*`, deltas) carry no `threadId`, so the reader maintains
//! `turn_id → thread_id` and `item_id → thread_id` maps populated from the
//! request registry and `item/started` events. Accepted limitation: when
//! two turns on different threads are truly in flight concurrently, item
//! deltas that arrive between them are attributed to the most-recently
//! started turn (see [`SharedReaderState::last_in_flight_thread`]).

use std::collections::{HashMap, VecDeque};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::codex_app_server::{self, AppServerEvent, ItemEvent, TurnStatus};
use super::*;

// ---------------------------------------------------------------------------
// MCP args construction
// ---------------------------------------------------------------------------

/// Build the `-c mcp_servers.chat.*` override flags for `codex app-server`.
///
/// Produces the native HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Returns a flat `Vec<String>`
/// ready to be extended onto the args list; each config override is already
/// preceded by its own `-c` flag.
///
/// Factored out so config-shape tests don't need a live bridge.
fn build_codex_mcp_args(bridge_endpoint: &str, token: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    let url_json = serde_json::to_string(&url).expect("url serialization cannot fail");
    args.push("-c".into());
    args.push(format!("mcp_servers.chat.url={url_json}"));
    args.push("-c".into());
    args.push("mcp_servers.chat.enabled=true".into());
    args.push("-c".into());
    args.push("mcp_servers.chat.required=true".into());

    args
}

// ---------------------------------------------------------------------------
// Transport abstraction — lets tests inject a fake stdin/stdout pair
// ---------------------------------------------------------------------------

/// Thin abstraction over the codex app-server transport. Production uses
/// [`SpawnedTransport`] (a real child process); tests inject a fake stdio
/// pair to drive the reader task without a binary.
trait CodexTransport: Send {
    /// Take the stdout reader half. Called exactly once by the reader task.
    fn take_stdout(&mut self) -> Box<dyn AsyncBufRead + Send + Unpin>;
    /// Take the stderr reader half. Called at most once.
    fn take_stderr(&mut self) -> Option<Box<dyn AsyncBufRead + Send + Unpin>>;
    /// Take the stdin writer half. Called exactly once. Returns an async
    /// `AsyncWrite` so the writer task can await flushes without occupying
    /// a blocking-pool slot (which would prevent tokio runtime teardown).
    fn take_stdin(&mut self) -> Box<dyn tokio::io::AsyncWrite + Send + Unpin>;
    /// Attempt to terminate the underlying process. No-op for fakes.
    fn terminate(&mut self);
}

/// Transport backed by a spawned `codex app-server` child process.
struct SpawnedTransport {
    child: Option<std::process::Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
}

impl CodexTransport for SpawnedTransport {
    fn take_stdout(&mut self) -> Box<dyn AsyncBufRead + Send + Unpin> {
        let stdout = self.stdout.take().expect("stdout taken twice");
        Box::new(BufReader::new(stdout))
    }

    fn take_stderr(&mut self) -> Option<Box<dyn AsyncBufRead + Send + Unpin>> {
        self.stderr
            .take()
            .map(|s| -> Box<dyn AsyncBufRead + Send + Unpin> { Box::new(BufReader::new(s)) })
    }

    fn take_stdin(&mut self) -> Box<dyn tokio::io::AsyncWrite + Send + Unpin> {
        Box::new(self.stdin.take().expect("stdin taken twice"))
    }

    fn terminate(&mut self) {
        if let Some(ref mut child) = self.child {
            let pid = child.id();
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CodexDriver — RuntimeDriver
// ---------------------------------------------------------------------------

/// Zero-size driver. The actual per-agent state lives in a
/// process-global map ([`agent_instances`]) so that the trait-object stored
/// in `manager.rs`'s registry (which is constructed via `Arc::new(CodexDriver)`)
/// does not have to carry state. All `CodexDriver` calls go through this
/// singleton.
pub struct CodexDriver;

/// Bound on `SharedReaderState::recent_turn_ids`. The deque feeds
/// `last_in_flight_thread()`, which attributes `item/*` notifications whose
/// underlying protocol frame carries no `threadId`. 32 is generous — in
/// practice very few turns are truly concurrent on one `codex app-server`.
const RECENT_TURN_IDS_CAP: usize = 32;

/// Process-global per-agent state map. Keyed by [`AgentKey`].
fn agent_instances() -> &'static AgentRegistry<CodexAgentProcess> {
    static INSTANCES: AgentRegistry<CodexAgentProcess> = AgentRegistry::new();
    &INSTANCES
}

impl CodexDriver {
    /// Look up or create the shared process state for an agent. The first
    /// call seeds the entry; subsequent `new_session`/`resume_session` calls
    /// return the same `Arc` so every handle writes into the same stdin and
    /// reads from the same event stream. Stale-entry eviction happens inside
    /// [`AgentRegistry::get_or_init`].
    fn ensure_process(&self, key: &AgentKey) -> Arc<CodexAgentProcess> {
        agent_instances().get_or_init(key, || {
            let (events, event_tx) = EventFanOut::new();
            Arc::new(CodexAgentProcess {
                key: key.clone(),
                events,
                event_tx,
                shared: Arc::new(Mutex::new(SharedReaderState::new())),
                stdin_tx: Mutex::new(None),
                next_request_id: AtomicU64::new(0),
                spawned: AtomicBool::new(false),
                child: Mutex::new(None),
                reader_handles: Mutex::new(Vec::new()),
            })
        })
    }
}

#[async_trait]
impl RuntimeDriver for CodexDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Codex
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("codex") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::CodexAppServer,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        let auth = run_command("codex", &["login", "status"])
            .ok()
            .map(|result| {
                let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
                if result.success && combined.contains("logged in") {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                }
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::CodexAppServer,
            capabilities: CapabilitySet::MODEL_LIST,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::Failed {
            reason: "codex does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("gpt-5.4".into()),
            ModelInfo::from_id("gpt-5.4-mini".into()),
            ModelInfo::from_id("gpt-5.3-codex".into()),
            ModelInfo::from_id("gpt-5.2-codex".into()),
            ModelInfo::from_id("gpt-5.2".into()),
            ModelInfo::from_id("gpt-5.1-codex-max".into()),
            ModelInfo::from_id("gpt-5.1-codex-mini".into()),
        ])
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    /// Allocates a [`CodexHandle`] for the given intent. For
    /// `SessionIntent::Resume(id)` the `resume_session_id` field is set,
    /// letting `run_inner` send `thread/resume` instead of `thread/start`.
    /// `session_id()` returns the pre-assigned id before `run()` fires.
    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let events = proc.events.clone();
        let mut handle = CodexHandle::new(key, spec, Arc::clone(&proc));
        if let SessionIntent::Resume(id) = intent {
            handle.resume_session_id = Some(id);
        }
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

}

// ---------------------------------------------------------------------------
// CodexAgentProcess — per-agent shared state
// ---------------------------------------------------------------------------

/// Shared `codex app-server` process. One instance per [`AgentKey`]; every
/// [`CodexHandle`] for that agent writes into the same stdin and reads from
/// the same event stream.
struct CodexAgentProcess {
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    /// Authoritative routing + session state. Reader task reads; handles
    /// mutate under the lock.
    shared: Arc<Mutex<SharedReaderState>>,
    /// Stdin writer channel. `None` until the first handle spawns the child.
    stdin_tx: Mutex<Option<mpsc::Sender<String>>>,
    /// Monotonic request id minter. `fetch_add` is the only write.
    next_request_id: AtomicU64,
    /// Whether the child process has been spawned. The first `start()`
    /// spawns it; later ones skip spawning.
    spawned: AtomicBool,
    /// Owns the transport so it is SIGTERM'd when the process drops.
    child: Mutex<Option<Box<dyn CodexTransport>>>,
    reader_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl CodexAgentProcess {
    /// Wire the stdin writer task and stdout reader task onto a shared
    /// process. Called by production after spawning a child, and by tests
    /// after injecting a fake transport. Takes `Arc<Self>` so the stdout
    /// reader task can own a clone of the process.
    fn wire_transport(this: &Arc<Self>, mut transport: Box<dyn CodexTransport>) {
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(128);
        {
            let mut guard = this.stdin_tx.lock().unwrap();
            *guard = Some(stdin_tx);
        }

        let mut writer = transport.take_stdin();
        let stdin_handle = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Some(line) = stdin_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        });

        let stdout = transport.take_stdout();
        let maybe_stderr = transport.take_stderr();

        // Stash the transport so we can SIGTERM on drop.
        {
            let mut guard = this.child.lock().unwrap();
            *guard = Some(transport);
        }

        let proc = Arc::clone(this);
        let stdout_handle = tokio::spawn(async move {
            reader_loop(proc, stdout).await;
        });

        let mut stderr_handle_opt = None;
        if let Some(stderr) = maybe_stderr {
            let key_err = this.key.clone();
            stderr_handle_opt = Some(tokio::spawn(async move {
                let mut lines = stderr.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        warn!(key = %key_err, line = %line, "codex stderr");
                    }
                }
            }));
        }

        let mut handles = this.reader_handles.lock().unwrap();
        handles.push(stdin_handle);
        handles.push(stdout_handle);
        if let Some(h) = stderr_handle_opt {
            handles.push(h);
        }
    }

    fn alloc_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a line on the shared stdin. Returns an error if the writer
    /// channel has not been initialized or if the reader task has exited.
    fn send_line(&self, line: String) -> anyhow::Result<()> {
        let tx = {
            let guard = self.stdin_tx.lock().unwrap();
            guard.clone()
        };
        let Some(tx) = tx else {
            bail!("codex: stdin not available — process not started");
        };
        tx.try_send(line)
            .context("codex: stdin channel full/closed")
    }

    fn emit(&self, event: DriverEvent) {
        super::emit_driver_event(
            &self.event_tx,
            event,
            &self.key,
            <Self as AgentProcess>::DRIVER_NAME,
        );
    }
}

impl AgentProcess for CodexAgentProcess {
    const DRIVER_NAME: &'static str = "codex";

    /// Returns `true` once the shared transport has gone away — either the
    /// child exited (writer task dropped the receiver) or the process was
    /// never wired up yet. The [`AgentRegistry`] evicts stale entries so a
    /// subsequent `attach` rebuilds from scratch.
    fn is_stale(&self) -> bool {
        if !self.spawned.load(Ordering::SeqCst) {
            // Never spawned — not stale, just fresh.
            return false;
        }
        let guard = self.stdin_tx.lock().unwrap();
        match guard.as_ref() {
            // Spawn flag was flipped but wiring hasn't landed yet — transient;
            // treat as live so we don't tear down a process mid-spawn.
            None => false,
            // Writer task exited (dropped the receiver) → child is dead.
            Some(tx) => tx.is_closed(),
        }
    }
}

impl Drop for CodexAgentProcess {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(ref mut transport) = *guard {
                transport.terminate();
            }
        }
        if let Ok(mut handles) = self.reader_handles.lock() {
            for h in handles.drain(..) {
                h.abort();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pending request registry
// ---------------------------------------------------------------------------

/// Recorded per-request context. Looked up by the stdout reader task when a
/// response arrives so it can classify by the method originally sent.
#[derive(Debug)]
struct PendingRequest {
    method: String,
    /// Thread id this request is scoped to, if known. `None` for
    /// `initialize` and fresh `thread/start`; `Some` for `thread/resume`
    /// and `turn/*`.
    thread_id: Option<String>,
    /// Optional waker fired when the response arrives. Used by code paths
    /// that need to block until a response lands (e.g. `thread/start` for
    /// a secondary session). Carries the classified event so the caller
    /// can inspect it.
    waker: Option<oneshot::Sender<AppServerEvent>>,
}

// ---------------------------------------------------------------------------
// Per-session state
// ---------------------------------------------------------------------------

/// Minimal per-session (per-thread) state tracked inside the shared reader
/// state. One entry per active session on the shared process.
#[derive(Debug, Clone)]
struct SessionState {
    /// Current lifecycle state.
    agent_state: AgentState,
    /// Turn id currently in flight on this session, if any.
    turn_id: Option<String>,
    /// Run id correlating the current prompt with its events.
    run_id: Option<RunId>,
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

/// State shared between every handle and the stdout reader task.
struct SharedReaderState {
    /// `true` once the `initialize` + `initialized` handshake has completed.
    initialized: bool,
    /// Wakers to fire when the initialize handshake finishes. Secondary
    /// handles park here before issuing `thread/start`.
    init_wakers: Vec<oneshot::Sender<()>>,
    /// Outstanding requests by id. Consumed on response arrival.
    pending_requests: HashMap<u64, PendingRequest>,
    /// `turn_id → thread_id`. Populated when `TurnResponse` is classified;
    /// drained on `turn/completed`.
    turn_to_thread: HashMap<String, String>,
    /// `item_id → thread_id`. Populated when `item/started` arrives — with
    /// best-effort attribution via the most-recently-started turn's thread.
    /// Codex notifications for items do not carry a threadId (see protocol
    /// docs), so when two turns on different threads are in flight
    /// concurrently this may misattribute a single item. Accepted for
    /// Stage 2; guidance is to avoid truly concurrent prompts on distinct
    /// sessions of one agent.
    item_to_thread: HashMap<String, String>,
    /// `session_id → state` for every thread this connection owns.
    sessions: HashMap<SessionId, SessionState>,
    /// FIFO of turn ids as we observe `TurnResponse` classifications. Used
    /// to attribute item/started notifications to the most-recently-started
    /// turn.
    recent_turn_ids: VecDeque<String>,
    /// Per-item command output buffer, keyed by item_id, capped at 256 KB each.
    cmd_output_buf: HashMap<String, String>,
}

impl SharedReaderState {
    fn new() -> Self {
        Self {
            initialized: false,
            init_wakers: Vec::new(),
            pending_requests: HashMap::new(),
            turn_to_thread: HashMap::new(),
            item_to_thread: HashMap::new(),
            sessions: HashMap::new(),
            recent_turn_ids: VecDeque::new(),
            cmd_output_buf: HashMap::new(),
        }
    }

    fn remember_turn(&mut self, turn_id: String, thread_id: String) {
        self.turn_to_thread.insert(turn_id.clone(), thread_id);
        self.recent_turn_ids.push_back(turn_id);
        // Keep the deque bounded so it doesn't grow unbounded across a long
        // session. See `RECENT_TURN_IDS_CAP`.
        while self.recent_turn_ids.len() > RECENT_TURN_IDS_CAP {
            self.recent_turn_ids.pop_front();
        }
    }

    /// Best-effort lookup of the most-recently-seen turn whose thread is
    /// still in flight. Used to attribute item/started notifications when
    /// the protocol does not carry a threadId.
    fn last_in_flight_thread(&self) -> Option<String> {
        for turn_id in self.recent_turn_ids.iter().rev() {
            if let Some(thread_id) = self.turn_to_thread.get(turn_id) {
                return Some(thread_id.clone());
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// CodexHandle — AgentSessionHandle
// ---------------------------------------------------------------------------

pub struct CodexHandle {
    key: AgentKey,
    /// Pre-start state only; after `start()` consult
    /// `shared.sessions[session_id].agent_state` instead.
    state: AgentState,
    spec: AgentSpec,
    process: Arc<CodexAgentProcess>,
    /// The `thread_id` this handle is attached to, filled by `start()`.
    session_id: Option<SessionId>,
    /// Set by `open_session(Resume(id))` to instruct `run_inner()` to send
    /// `thread/resume` instead of `thread/start`.
    resume_session_id: Option<SessionId>,
}

impl CodexHandle {
    fn new(key: AgentKey, spec: AgentSpec, process: Arc<CodexAgentProcess>) -> Self {
        Self {
            key,
            state: AgentState::Idle,
            spec,
            process,
            session_id: None,
            resume_session_id: None,
        }
    }

    /// Ensure the `codex app-server` child process is spawned and the
    /// initialize handshake has completed. If this is the first handle on
    /// the agent, spawn the process and run the handshake; otherwise park
    /// until another handle finishes initializing.
    async fn ensure_process_started(&self) -> anyhow::Result<()> {
        // Race to be the spawner. Exactly one handle wins and performs the
        // spawn; the rest fall through to the init-waiter path below.
        let should_spawn = !self.process.spawned.swap(true, Ordering::SeqCst);

        if should_spawn {
            self.spawn_child_process().await?;
        }

        // Park until initialize completes. The spawner fires all wakers
        // after the response arrives.
        let (tx, rx) = oneshot::channel::<()>();
        let needs_wait = {
            let mut s = self.process.shared.lock().unwrap();
            if s.initialized {
                false
            } else {
                s.init_wakers.push(tx);
                true
            }
        };
        if needs_wait {
            rx.await
                .context("codex: initialize handshake aborted before completion")?;
        } else {
            drop(rx);
        }
        Ok(())
    }

    /// Spawn the child process. Called exactly once per agent.
    async fn spawn_child_process(&self) -> anyhow::Result<()> {
        let args = self.build_child_args().await?;
        let mut cmd = Command::new("codex");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn codex")?;
        let stdin_raw = child.stdin.take().context("codex: missing stdin")?;
        let stdout_raw = child.stdout.take().context("codex: missing stdout")?;
        let stderr_raw = child.stderr.take().context("codex: missing stderr")?;

        let stdin_async =
            tokio::process::ChildStdin::from_std(stdin_raw).context("codex: convert stdin")?;
        let stdout_async =
            tokio::process::ChildStdout::from_std(stdout_raw).context("codex: convert stdout")?;
        let stderr_async =
            tokio::process::ChildStderr::from_std(stderr_raw).context("codex: convert stderr")?;

        let transport = SpawnedTransport {
            child: Some(child),
            stdin: Some(stdin_async),
            stdout: Some(stdout_async),
            stderr: Some(stderr_async),
        };

        self.wire_transport(Box::new(transport));

        // Kick off the initialize handshake.
        let init_req_id = self.process.alloc_request_id();
        {
            let mut s = self.process.shared.lock().unwrap();
            s.pending_requests.insert(
                init_req_id,
                PendingRequest {
                    method: "initialize".to_string(),
                    thread_id: None,
                    waker: None,
                },
            );
        }
        let init_req = codex_app_server::build_initialize(init_req_id);
        self.process
            .send_line(init_req)
            .context("codex: failed to queue initialize request")?;

        Ok(())
    }

    /// Wire the stdin writer task and stdout reader task onto the shared
    /// process. Called by production after spawning a child, and by tests
    /// after injecting a fake transport.
    fn wire_transport(&self, transport: Box<dyn CodexTransport>) {
        CodexAgentProcess::wire_transport(&self.process, transport);
    }

    /// Build the CLI args `codex` is invoked with: `app-server` plus the
    /// `-c mcp_servers.chat.*` overrides pointing at the shared bridge.
    async fn build_child_args(&self) -> anyhow::Result<Vec<String>> {
        let mut args: Vec<String> = vec!["app-server".into()];
        let token = super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
            .await
            .context("failed to pair with shared bridge")?;
        let mcp_args = build_codex_mcp_args(&self.spec.bridge_endpoint, &token);
        args.extend(mcp_args);
        Ok(args)
    }

    /// Issue `thread/start` or `thread/resume` on the shared stdin and
    /// block until the response lands. Returns the thread id the server
    /// assigned (or confirmed, for resume).
    async fn start_or_resume_thread(&self, resume_id: Option<String>) -> anyhow::Result<String> {
        let req_id = self.process.alloc_request_id();
        let (method, req_line) = match &resume_id {
            Some(tid) => (
                "thread/resume".to_string(),
                codex_app_server::build_thread_resume(req_id, tid),
            ),
            None => (
                "thread/start".to_string(),
                codex_app_server::build_thread_start(
                    req_id,
                    &self.spec.model,
                    &self.spec.working_directory.to_string_lossy(),
                    self.spec.system_prompt.as_deref(),
                ),
            ),
        };

        let (tx, rx) = oneshot::channel::<AppServerEvent>();
        {
            let mut s = self.process.shared.lock().unwrap();
            s.pending_requests.insert(
                req_id,
                PendingRequest {
                    method,
                    thread_id: resume_id.clone(),
                    waker: Some(tx),
                },
            );
        }

        self.process
            .send_line(req_line)
            .context("codex: failed to queue thread request")?;

        let ev = rx
            .await
            .context("codex: thread request dropped before response")?;

        match ev {
            AppServerEvent::ThreadResponse { thread_id } => Ok(thread_id),
            AppServerEvent::Error { message, .. } => {
                bail!("codex: thread request rejected: {message}")
            }
            other => bail!("codex: unexpected response to thread request: {other:?}"),
        }
    }

    /// Core session-start logic. Reads `self.resume_session_id` (set by
    /// `open_session(Resume)` or the `start` compat shim) to decide whether
    /// to send `thread/resume` or `thread/start` to the app-server.
    async fn run_inner(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        if !matches!(self.state, AgentState::Idle) {
            bail!("codex: start called in non-idle state");
        }

        self.state = AgentState::Starting;
        self.process.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Spawn the shared child process if we're the first handle on this
        // agent; otherwise wait for the initialize handshake to finish.
        self.ensure_process_started().await?;

        // Use the native resume_session_id field set by open_session(Resume)
        // or the start compat shim.
        let resume_id = self.resume_session_id.take();
        let thread_id = self.start_or_resume_thread(resume_id).await?;
        self.session_id = Some(thread_id.clone());

        // Seed the per-session state row and emit Active lifecycle.
        {
            let mut s = self.process.shared.lock().unwrap();
            s.sessions.insert(
                thread_id.clone(),
                SessionState {
                    agent_state: AgentState::Active {
                        session_id: thread_id.clone(),
                    },
                    turn_id: None,
                    run_id: None,
                },
            );
        }
        self.process.emit(DriverEvent::SessionAttached {
            key: self.key.clone(),
            session_id: thread_id.clone(),
        });
        self.process.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Active {
                session_id: thread_id.clone(),
            },
        });
        self.state = AgentState::Active {
            session_id: thread_id.clone(),
        };

        // Deliver the deferred first prompt (if any).
        if let Some(req) = init_prompt {
            self.prompt(req).await?;
        }

        Ok(())
    }
}

#[async_trait]
impl AgentSessionHandle for CodexHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn session_id(&self) -> Option<&str> {
        // After run() completes, self.session_id holds the thread id assigned
        // (or confirmed) by the server. Before run(), fall back to
        // resume_session_id so callers that call open_session(Resume(id)) can
        // read the intent back immediately.
        self.session_id
            .as_deref()
            .or(self.resume_session_id.as_deref())
    }

    fn state(&self) -> AgentState {
        if let Some(ref sid) = self.session_id {
            let s = self.process.shared.lock().unwrap();
            if let Some(session) = s.sessions.get(sid) {
                return session.agent_state.clone();
            }
        }
        self.state.clone()
    }

    /// Native `run`: reads `resume_session_id` stored by `open_session(Resume)`
    /// and delegates to `run_inner`. For `open_session(New)` the field is `None`
    /// and `run_inner` starts a fresh thread.
    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.run_inner(init_prompt).await
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("codex: cannot prompt — handle not started"))?;

        // Guard: must be Active (not already PromptInFlight).
        {
            let s = self.process.shared.lock().unwrap();
            let session = s
                .sessions
                .get(&session_id)
                .ok_or_else(|| anyhow::anyhow!("codex: session state missing for {session_id}"))?;
            if !matches!(session.agent_state, AgentState::Active { .. }) {
                bail!("codex: cannot prompt in non-active state");
            }
        }

        let run_id = RunId::new_v4();
        let req_id = self.process.alloc_request_id();

        {
            let mut s = self.process.shared.lock().unwrap();
            s.pending_requests.insert(
                req_id,
                PendingRequest {
                    method: "turn/start".to_string(),
                    thread_id: Some(session_id.clone()),
                    waker: None,
                },
            );
            if let Some(session) = s.sessions.get_mut(&session_id) {
                session.run_id = Some(run_id);
                session.agent_state = AgentState::PromptInFlight {
                    run_id,
                    session_id: session_id.clone(),
                };
            }
        }

        self.process.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let turn_req = codex_app_server::build_turn_start(req_id, &session_id, &req.text);
        self.process
            .send_line(turn_req)
            .context("codex: stdin channel closed")?;

        self.state = AgentState::PromptInFlight { run_id, session_id };
        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        let Some(session_id) = self.session_id.clone() else {
            return Ok(CancelOutcome::NotInFlight);
        };

        let (run_id, turn_id) = {
            let mut s = self.process.shared.lock().unwrap();
            let Some(session) = s.sessions.get_mut(&session_id) else {
                return Ok(CancelOutcome::NotInFlight);
            };
            if !matches!(session.agent_state, AgentState::PromptInFlight { .. }) {
                return Ok(CancelOutcome::NotInFlight);
            }
            let run_id = session.run_id.take();
            let turn_id = session.turn_id.take();
            session.agent_state = AgentState::Active {
                session_id: session_id.clone(),
            };
            (run_id, turn_id)
        };

        // Send a real turn/interrupt if we have the turn id.
        if let Some(vid) = turn_id {
            let req_id = self.process.alloc_request_id();
            {
                let mut s = self.process.shared.lock().unwrap();
                s.pending_requests.insert(
                    req_id,
                    PendingRequest {
                        method: "turn/interrupt".to_string(),
                        thread_id: Some(session_id.clone()),
                        waker: None,
                    },
                );
            }
            let interrupt = codex_app_server::build_turn_interrupt(req_id, &session_id, &vid);
            let _ = self.process.send_line(interrupt);
        }

        // Emit synthetic completion so callers aren't left waiting.
        if let Some(run_id) = run_id {
            self.process.emit(DriverEvent::Completed {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Cancelled,
                },
            });
        }

        self.state = AgentState::Active { session_id };
        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, AgentState::Closed) {
            return Ok(());
        }

        // Mark this handle's session as Closed in shared state. If we were
        // the last live session on this agent — AND no new_session /
        // resume_session is mid-flight (which would add a new session
        // slot on response) — also drop the registry entry so the shared
        // `Arc<CodexAgentProcess>` refcount can reach zero and its Drop impl
        // terminates the child process + reader tasks.
        let last_session = {
            let mut s = self.process.shared.lock().unwrap();
            if let Some(ref sid) = self.session_id {
                if let Some(session) = s.sessions.get_mut(sid) {
                    session.agent_state = AgentState::Closed;
                }
            }
            let all_closed = s
                .sessions
                .values()
                .all(|sess| matches!(sess.agent_state, AgentState::Closed));
            // Don't tear down while a thread/start or thread/resume response
            // is pending — the caller is awaiting a new session that would
            // lose its backing process if we pruned now.
            let no_pending_session_creation = !s
                .pending_requests
                .values()
                .any(|pr| pr.method == "thread/start" || pr.method == "thread/resume");
            all_closed && no_pending_session_creation
        };

        if last_session {
            // Remove the registry entry. The driver map was holding one of
            // the Arc refs; other refs (on live `CodexHandle`s) keep the
            // process alive until every handle is dropped. Once they are,
            // `Drop for CodexAgentProcess` fires: SIGTERM to the child +
            // abort for the reader tasks.
            agent_instances().remove(&self.key);
        }

        self.state = AgentState::Closed;

        self.process.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader task
// ---------------------------------------------------------------------------

/// Consume lines from the child process's stdout and dispatch events to the
/// shared fan-out. Owns response classification (via the request registry)
/// and per-session state transitions.
async fn reader_loop(proc: Arc<CodexAgentProcess>, stdout: Box<dyn AsyncBufRead + Send + Unpin>) {
    let mut lines = stdout.lines();

    let emit = |ev: DriverEvent| {
        if let Err(e) = proc.event_tx.try_send(ev) {
            warn!("codex v2: dropped event in reader: {e}");
        }
    };

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        trace!(line = %line, "codex stdout");

        // Extract the pending request (if any) during classification so we
        // can wake blocked callers after.
        let mut pending_out: Option<PendingRequest> = None;
        let classified = {
            let proc_ref = &proc;
            let pending_slot = &mut pending_out;
            codex_app_server::parse_line_with_registry(&line, |id| {
                let mut s = proc_ref.shared.lock().unwrap();
                if let Some(pr) = s.pending_requests.remove(&id) {
                    let method = pr.method.clone();
                    *pending_slot = Some(pr);
                    Some(method)
                } else {
                    None
                }
            })
        };

        // Update routing + fire waker (if any) based on what classification returned.
        if let Some(mut pr) = pending_out.take() {
            update_routing_from_response(&proc, &pr, &classified);
            if let Some(waker) = pr.waker.take() {
                let _ = waker.send(classified.clone());
            }
        }

        handle_event(&proc, classified, &emit).await;
    }

    // EOF on stdout — the process exited. Surface TransportClosed for every
    // in-flight session and flip them to Closed.
    let sessions_to_finish: Vec<(SessionId, Option<RunId>)> = {
        let s = proc.shared.lock().unwrap();
        s.sessions
            .iter()
            .map(|(sid, state)| (sid.clone(), state.run_id))
            .collect()
    };
    for (session_id, run_id_opt) in sessions_to_finish {
        if let Some(run_id) = run_id_opt {
            emit(DriverEvent::Completed {
                key: proc.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::TransportClosed,
                },
            });
        }
        emit(DriverEvent::Lifecycle {
            key: proc.key.clone(),
            state: AgentState::Closed,
        });
        let mut s = proc.shared.lock().unwrap();
        if let Some(session) = s.sessions.get_mut(&session_id) {
            session.agent_state = AgentState::Closed;
            session.run_id = None;
        }
    }
}

/// Post-classification routing hook — learns `turn_id → thread_id` and
/// drives the post-initialize handshake.
fn update_routing_from_response(
    proc: &CodexAgentProcess,
    pr: &PendingRequest,
    ev: &AppServerEvent,
) {
    match (pr.method.as_str(), ev) {
        ("turn/start", AppServerEvent::TurnResponse { turn_id }) => {
            if let Some(ref tid) = pr.thread_id {
                let mut s = proc.shared.lock().unwrap();
                s.remember_turn(turn_id.clone(), tid.clone());
                if let Some(session) = s.sessions.get_mut(tid) {
                    session.turn_id = Some(turn_id.clone());
                }
            }
        }
        ("initialize", AppServerEvent::InitializeResponse) => {
            // Send the `initialized` notification, flip the init flag, fire wakers.
            let initialized = codex_app_server::build_initialized();
            let _ = proc.send_line(initialized);
            let wakers = {
                let mut s = proc.shared.lock().unwrap();
                s.initialized = true;
                std::mem::take(&mut s.init_wakers)
            };
            for w in wakers {
                let _ = w.send(());
            }
        }
        ("initialize", AppServerEvent::Error { message, code, .. }) => {
            // Initialize failed. Drop every parked init waker so their
            // `rx.await` resolves with `RecvError` — `ensure_process_started`
            // converts that into an anyhow error via `.context(...)`. Without
            // this the wakers would sit in the map forever and every handle
            // that called `ensure_process_started` would block indefinitely.
            warn!(
                code = code,
                message = %message,
                "codex: initialize handshake failed; dropping init wakers"
            );
            let wakers = {
                let mut s = proc.shared.lock().unwrap();
                std::mem::take(&mut s.init_wakers)
            };
            drop(wakers);
        }
        _ => {}
    }
}

/// Process one parsed (post-classification) event: update session state and
/// fan out driver events.
async fn handle_event<F: Fn(DriverEvent)>(
    proc: &Arc<CodexAgentProcess>,
    ev: AppServerEvent,
    emit: &F,
) {
    match ev {
        AppServerEvent::InitializeResponse => {
            // Handled in update_routing_from_response.
        }
        AppServerEvent::ThreadResponse { .. } => {
            // Consumed by the request waker in `start_or_resume_thread`.
        }
        AppServerEvent::TurnResponse { turn_id: _ } => {
            // Already folded into routing tables; nothing user-facing.
        }
        AppServerEvent::TurnInterruptResponse => {
            debug!("codex: turn interrupt acknowledged");
        }

        AppServerEvent::TurnStarted { turn_id } => {
            debug!("codex: turn started; turn_id = {}", turn_id);
        }

        AppServerEvent::TurnCompleted { turn_id, status } => {
            let (thread_id, run_id) = {
                let mut s = proc.shared.lock().unwrap();
                let thread_id = s.turn_to_thread.remove(&turn_id).unwrap_or_default();
                // Drain command-output entries that belong to this turn's
                // thread. An older implementation called `cmd_output_buf.clear()`
                // unconditionally here, which wiped buffers owned by sibling
                // sessions whose turns were still in flight — a multi-session
                // correctness bug. Now we only release items attributed to the
                // completing thread (ItemCompleted already removes cleanly on
                // the happy path; this is belt-and-suspenders cleanup for
                // items whose completion we never saw).
                if !thread_id.is_empty() {
                    let drop_items: Vec<String> = s
                        .item_to_thread
                        .iter()
                        .filter(|(_, t)| t.as_str() == thread_id.as_str())
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in &drop_items {
                        s.cmd_output_buf.remove(id);
                        s.item_to_thread.remove(id);
                    }
                }
                let run_id = if !thread_id.is_empty() {
                    if let Some(session) = s.sessions.get_mut(&thread_id) {
                        let rid = session.run_id.take();
                        session.turn_id = None;
                        if rid.is_some() {
                            session.agent_state = AgentState::Active {
                                session_id: thread_id.clone(),
                            };
                        }
                        rid
                    } else {
                        None
                    }
                } else {
                    None
                };
                (thread_id, run_id)
            };

            if let Some(run_id) = run_id {
                let finish_reason = match &status {
                    TurnStatus::Completed => FinishReason::Natural,
                    TurnStatus::Interrupted => FinishReason::Cancelled,
                    TurnStatus::Failed { message } => {
                        emit(DriverEvent::Output {
                            key: proc.key.clone(),
                            session_id: thread_id.clone(),
                            run_id,
                            item: AgentEventItem::Text {
                                text: format!("⚠️ {message}"),
                            },
                        });
                        FinishReason::Natural
                    }
                };
                emit(DriverEvent::Output {
                    key: proc.key.clone(),
                    session_id: thread_id.clone(),
                    run_id,
                    item: AgentEventItem::TurnEnd,
                });
                emit(DriverEvent::Completed {
                    key: proc.key.clone(),
                    session_id: thread_id.clone(),
                    run_id,
                    result: RunResult { finish_reason },
                });
                emit(DriverEvent::Lifecycle {
                    key: proc.key.clone(),
                    state: AgentState::Active {
                        session_id: thread_id,
                    },
                });
            }
        }

        AppServerEvent::AgentMessageDelta { item_id, text } => {
            let (run_id, thread_id) = resolve_item_target(proc, &item_id);
            if let Some(run_id) = run_id {
                emit(DriverEvent::Output {
                    key: proc.key.clone(),
                    session_id: thread_id,
                    run_id,
                    item: AgentEventItem::Text { text },
                });
            }
        }

        AppServerEvent::ReasoningSummaryDelta { item_id, text } => {
            let (run_id, thread_id) = resolve_item_target(proc, &item_id);
            if let Some(run_id) = run_id {
                emit(DriverEvent::Output {
                    key: proc.key.clone(),
                    session_id: thread_id,
                    run_id,
                    item: AgentEventItem::Thinking { text },
                });
            }
        }

        AppServerEvent::CommandOutputDelta { item_id, text } => {
            // Buffer up to 256 KB per command item; still forward each delta.
            // Drained at TurnCompleted.
            const MAX_BUF: usize = 256 * 1024;
            {
                let mut s = proc.shared.lock().unwrap();
                let buf = s.cmd_output_buf.entry(item_id.clone()).or_default();
                if buf.len() + text.len() <= MAX_BUF {
                    buf.push_str(&text);
                }
            }
            let (run_id, thread_id) = resolve_item_target(proc, &item_id);
            if let Some(run_id) = run_id {
                emit(DriverEvent::Output {
                    key: proc.key.clone(),
                    session_id: thread_id,
                    run_id,
                    item: AgentEventItem::Text { text },
                });
            }
        }

        AppServerEvent::CommandApproval { request_id, .. } => {
            // approval_policy=never should prevent these; approve defensively.
            let resp = codex_app_server::build_approval_response(&request_id, "accept");
            let _ = proc.send_line(resp);
            debug!("codex: auto-approved command execution");
        }

        AppServerEvent::FileChangeApproval { request_id, .. } => {
            let resp = codex_app_server::build_approval_response(&request_id, "accept");
            let _ = proc.send_line(resp);
            debug!("codex: auto-approved file change");
        }

        AppServerEvent::Error { message, .. } => {
            warn!(message = %message, "codex: protocol error");
            let (run_id, thread_id) = {
                let mut s = proc.shared.lock().unwrap();
                let thread_id = s.last_in_flight_thread().unwrap_or_default();
                let run_id = if !thread_id.is_empty() {
                    s.sessions
                        .get_mut(&thread_id)
                        .and_then(|sess| sess.run_id.take())
                } else {
                    None
                };
                (run_id, thread_id)
            };
            if let Some(run_id) = run_id {
                emit(DriverEvent::Failed {
                    key: proc.key.clone(),
                    session_id: thread_id,
                    run_id,
                    error: AgentError::RuntimeReported(message),
                });
            }
        }

        AppServerEvent::ItemStarted { ref item } => {
            // Record item_id → thread_id so deltas keyed by itemId route to
            // the right session. Attribution uses the most-recently-seen
            // in-flight turn's thread (see SharedReaderState::last_in_flight_thread
            // for the accepted limitation).
            let item_id = item_id_of(item);
            if let Some(id) = item_id {
                let thread_id = {
                    let s = proc.shared.lock().unwrap();
                    s.last_in_flight_thread()
                };
                if let Some(tid) = thread_id {
                    let mut s = proc.shared.lock().unwrap();
                    s.item_to_thread.insert(id, tid);
                }
            }
        }

        // ItemCompleted: emit ToolCall/ToolResult trace events.
        AppServerEvent::ItemCompleted { item } => {
            let item_id = item_id_of(&item);
            let (run_id, thread_id) = {
                let mut s = proc.shared.lock().unwrap();
                let thread_id = item_id
                    .as_ref()
                    .and_then(|id| s.item_to_thread.remove(id))
                    .or_else(|| s.last_in_flight_thread())
                    .unwrap_or_default();
                let run_id = s.sessions.get(&thread_id).and_then(|sess| sess.run_id);
                (run_id, thread_id)
            };
            if let Some(run_id) = run_id {
                match item {
                    ItemEvent::CommandExecution {
                        id,
                        command,
                        exit_code,
                        ..
                    } => {
                        let output = {
                            let s = proc.shared.lock().unwrap();
                            s.cmd_output_buf.get(&id).cloned().unwrap_or_default()
                        };
                        emit(DriverEvent::Output {
                            key: proc.key.clone(),
                            session_id: thread_id.clone(),
                            run_id,
                            item: AgentEventItem::ToolCall {
                                name: "shell".to_string(),
                                input: serde_json::json!({ "command": command }),
                            },
                        });
                        let result = match exit_code {
                            Some(code) if !output.is_empty() => {
                                format!("(exit {code}) {output}")
                            }
                            Some(code) => format!("exit_code={code}"),
                            None => output,
                        };
                        emit(DriverEvent::Output {
                            key: proc.key.clone(),
                            session_id: thread_id,
                            run_id,
                            item: AgentEventItem::ToolResult { content: result },
                        });
                    }
                    ItemEvent::McpToolCall {
                        server,
                        tool,
                        arguments,
                        ..
                    } => {
                        emit(DriverEvent::Output {
                            key: proc.key.clone(),
                            session_id: thread_id,
                            run_id,
                            item: AgentEventItem::ToolCall {
                                name: format!("{server}/{tool}"),
                                input: arguments,
                            },
                        });
                    }
                    _ => {}
                }
            }
        }

        // Informational notifications — no action required
        AppServerEvent::ThreadStarted { .. } | AppServerEvent::Unknown => {}
    }
}

/// Resolve the `(run_id, thread_id)` pair for an item-keyed event. First
/// consults `item_to_thread`; falls back to the most-recently-started
/// in-flight turn's thread.
fn resolve_item_target(proc: &CodexAgentProcess, item_id: &str) -> (Option<RunId>, String) {
    let s = proc.shared.lock().unwrap();
    let thread_id = s
        .item_to_thread
        .get(item_id)
        .cloned()
        .or_else(|| s.last_in_flight_thread())
        .unwrap_or_default();
    let run_id = s.sessions.get(&thread_id).and_then(|sess| sess.run_id);
    (run_id, thread_id)
}

/// Extract the `id` field of an `ItemEvent` regardless of variant.
fn item_id_of(item: &ItemEvent) -> Option<String> {
    let id = match item {
        ItemEvent::AgentMessage { id, .. } => id,
        ItemEvent::Reasoning { id, .. } => id,
        ItemEvent::CommandExecution { id, .. } => id,
        ItemEvent::FileChange { id, .. } => id,
        ItemEvent::McpToolCall { id, .. } => id,
        ItemEvent::UserMessage { id } => id,
        ItemEvent::Other { id, .. } => id,
    };
    if id.is_empty() {
        None
    } else {
        Some(id.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-codex".into(),
            description: None,
            system_prompt: None,
            model: "gpt-5.4".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".into(),
        }
    }

    #[tokio::test]
    async fn test_codex_driver_probe_not_installed() {
        let driver = CodexDriver;
        let probe = driver.probe().await.unwrap();
        // When the codex binary is absent the transport must be CodexAppServer
        assert_eq!(probe.transport, TransportKind::CodexAppServer);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
    }

    #[tokio::test]
    async fn test_codex_driver_list_models() {
        let driver = CodexDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 7);
        assert_eq!(models[0].id, "gpt-5.4");
        assert_eq!(models[1].id, "gpt-5.4-mini");
        assert_eq!(models[2].id, "gpt-5.3-codex");
        assert_eq!(models[3].id, "gpt-5.2-codex");
        assert_eq!(models[4].id, "gpt-5.2");
        assert_eq!(models[5].id, "gpt-5.1-codex-max");
        assert_eq!(models[6].id, "gpt-5.1-codex-mini");
    }

    #[tokio::test]
    async fn test_codex_driver_open_session_returns_idle() {
        let driver = CodexDriver;
        let result = driver
            .open_session("agent-codex-1".into(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    #[tokio::test]
    async fn test_codex_handle_shared_is_none_before_run() {
        // Before run(), state() falls back to self.state which is Idle.
        let driver = CodexDriver;
        let result = driver
            .open_session("agent-codex-3".into(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    // ---- build_codex_mcp_args tests ----

    #[test]
    fn build_codex_mcp_args_http_shape() {
        let args = build_codex_mcp_args("http://127.0.0.1:4321", "tok-xyz");

        let joined = args.join(" ");
        assert!(
            joined.contains("mcp_servers.chat.url="),
            "expected url override, got: {joined}"
        );
        assert!(
            joined.contains("tok-xyz"),
            "expected token in url, got: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.command="),
            "unexpected command override in http path: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.args="),
            "unexpected args override in http path: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.type="),
            "unexpected type override in http path (url implies transport): {joined}"
        );
        assert!(joined.contains("mcp_servers.chat.enabled=true"));
        assert!(joined.contains("mcp_servers.chat.required=true"));
        for i in (0..args.len()).step_by(2) {
            assert_eq!(args[i], "-c", "expected -c at index {i}, got: {}", args[i]);
        }
        let url_arg = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.chat.url="))
            .expect("url arg not found");
        let json_val = url_arg.trim_start_matches("mcp_servers.chat.url=");
        let decoded: String =
            serde_json::from_str(json_val).expect("url value should be JSON string");
        assert_eq!(decoded, "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }

    #[test]
    fn build_codex_mcp_args_trims_trailing_slash() {
        let args = build_codex_mcp_args("http://127.0.0.1:4321/", "tok-xyz");

        let url_arg = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.chat.url="))
            .expect("url arg not found");
        let json_val = url_arg.trim_start_matches("mcp_servers.chat.url=");
        let decoded: String =
            serde_json::from_str(json_val).expect("url value should be JSON string");
        assert_eq!(decoded, "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert!(
            !decoded.contains("//token/"),
            "double-slash in URL: {decoded}"
        );
    }

    // ---- ensure_process: shared process invariant ----

    #[tokio::test]
    async fn open_session_calls_share_process() {
        // Proves that three open_session calls on the same key all reference
        // the same CodexAgentProcess — a.k.a. "one agent, one child".
        // We don't actually spawn a child in this test (no real `codex`
        // binary); we only inspect the bookkeeping.
        let driver = CodexDriver;
        let key = "agent-share-probe".to_string();
        let _a = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let _b = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let _c = driver
            .open_session(
                key.clone(),
                test_spec(),
                SessionIntent::Resume("thr_stored".into()),
            )
            .await
            .unwrap();

        // All four Arcs (registry + 3 handles) should point at the same
        // CodexAgentProcess — observed via strong_count == 4 on the driver
        // map entry (driver holds one, each handle holds one).
        // Use the raw-lock escape hatch: `get()` would clone the Arc and
        // bump the strong count by one, breaking the count assertion.
        let guard = agent_instances().lock();
        let arc = guard.get(&key).expect("agent instance recorded");
        assert_eq!(
            Arc::strong_count(arc),
            4,
            "registry + 3 handles must share one process"
        );
    }
}

// ---------------------------------------------------------------------------
// Multi-session integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod multisession_tests {
    //! End-to-end multi-session tests. Exercise `new_session` and
    //! `resume_session` without spawning a real `codex` binary — a fake
    //! transport drives the reader task with canned JSON-RPC lines that
    //! simulate the app-server response pattern (`initialize` →
    //! `thread/start`/`thread/resume` → `turn/start`).

    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::io::{duplex, AsyncWriteExt, DuplexStream};
    use tokio::sync::Mutex as TokioMutex;
    use tokio::time::timeout;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-codex".into(),
            description: None,
            system_prompt: None,
            model: "gpt-5.4".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".into(),
        }
    }

    /// In-memory transport driving the reader task from canned lines. The
    /// writer half routes stdin writes to a `DuplexStream` so a simulator
    /// task can inspect requests and emit matching responses on stdout.
    struct FakeTransport {
        stdout_reader: Option<Box<dyn AsyncBufRead + Send + Unpin>>,
        stdin_writer: Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>,
    }

    impl CodexTransport for FakeTransport {
        fn take_stdout(&mut self) -> Box<dyn AsyncBufRead + Send + Unpin> {
            self.stdout_reader.take().expect("stdout taken twice")
        }
        fn take_stderr(&mut self) -> Option<Box<dyn AsyncBufRead + Send + Unpin>> {
            None
        }
        fn take_stdin(&mut self) -> Box<dyn tokio::io::AsyncWrite + Send + Unpin> {
            self.stdin_writer.take().expect("stdin taken twice")
        }
        fn terminate(&mut self) {}
    }

    /// Fake app-server simulator. Owns the stdout writer half of the
    /// in-memory transport and responds to each stdin line with the
    /// corresponding JSON-RPC response.
    struct Simulator {
        _sim_handle: tokio::task::JoinHandle<()>,
        thread_ids_assigned: Arc<std::sync::Mutex<Vec<String>>>,
        #[allow(dead_code)]
        turn_ids_assigned: Arc<std::sync::Mutex<Vec<String>>>,
    }

    /// Install a fake transport on a shared [`CodexAgentProcess`]. Flips
    /// the process's `spawned` flag so `ensure_process_started` doesn't
    /// attempt to spawn a real child, then wires in the fake transport and
    /// returns a simulator that drives the server-side of the protocol.
    fn install_fake_transport(proc: &Arc<CodexAgentProcess>) -> Simulator {
        // stdout pipe: simulator writes → handle reads.
        let (stdout_writer, stdout_reader): (DuplexStream, DuplexStream) = duplex(64 * 1024);
        let stdout_reader_buf: Box<dyn AsyncBufRead + Send + Unpin> =
            Box::new(BufReader::new(stdout_reader));

        // stdin pipe: handle writes → simulator reads. Fully async so
        // the tokio runtime can tear the test down cleanly.
        let (stdin_writer, stdin_reader): (DuplexStream, DuplexStream) = duplex(64 * 1024);

        let transport = FakeTransport {
            stdout_reader: Some(stdout_reader_buf),
            stdin_writer: Some(Box::new(stdin_writer)),
        };

        // Flip the `spawned` flag AND mark the init handshake as already
        // completed. In production, the first start() spawns the child
        // and drives initialize → response → initialized. In tests we
        // short-circuit that: the simulator task ignores any `initialize`
        // line anyway.
        proc.spawned
            .store(true, std::sync::atomic::Ordering::SeqCst);
        {
            let mut s = proc.shared.lock().unwrap();
            s.initialized = true;
        }
        CodexAgentProcess::wire_transport(proc, Box::new(transport));

        let thread_ids = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let turn_ids = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let thread_ids_cl = Arc::clone(&thread_ids);
        let turn_ids_cl = Arc::clone(&turn_ids);

        // Wrap the stdout writer in a tokio::Mutex so the spawned
        // simulator task can asynchronously write responses.
        let stdout_writer = Arc::new(TokioMutex::new(stdout_writer));

        let sim_handle = tokio::spawn(async move {
            let stdin_reader = BufReader::new(stdin_reader);
            let mut lines = stdin_reader.lines();
            let mut next_thr = 1usize;
            let mut next_turn = 1usize;

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let id = v.get("id").cloned();

                let response_line = match method {
                    "initialize" => {
                        let resp = serde_json::json!({
                            "id": id,
                            "result": {
                                "protocolVersion": 1,
                                "serverInfo": {"name": "codex"}
                            }
                        });
                        Some(resp.to_string())
                    }
                    "thread/start" => {
                        let tid = format!("thr_{next_thr}");
                        next_thr += 1;
                        thread_ids_cl.lock().unwrap().push(tid.clone());
                        let resp = serde_json::json!({
                            "id": id,
                            "result": { "thread": { "id": tid } }
                        });
                        Some(resp.to_string())
                    }
                    "thread/resume" => {
                        let tid = v
                            .get("params")
                            .and_then(|p| p.get("threadId"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        thread_ids_cl.lock().unwrap().push(tid.clone());
                        let resp = serde_json::json!({
                            "id": id,
                            "result": { "thread": { "id": tid } }
                        });
                        Some(resp.to_string())
                    }
                    "turn/start" => {
                        let tid = format!("turn_{next_turn}");
                        next_turn += 1;
                        turn_ids_cl.lock().unwrap().push(tid.clone());
                        let resp = serde_json::json!({
                            "id": id,
                            "result": {
                                "turn": {
                                    "id": tid,
                                    "status": "inProgress",
                                    "items": [],
                                    "error": null
                                }
                            }
                        });
                        Some(resp.to_string())
                    }
                    "turn/interrupt" => {
                        let resp = serde_json::json!({
                            "id": id,
                            "result": {}
                        });
                        Some(resp.to_string())
                    }
                    // "initialized" notification has no id; nothing to reply.
                    _ => None,
                };

                if let Some(line) = response_line {
                    let mut w = stdout_writer.lock().await;
                    let _ = w.write_all(line.as_bytes()).await;
                    let _ = w.write_all(b"\n").await;
                    let _ = w.flush().await;
                }
            }
        });

        Simulator {
            _sim_handle: sim_handle,
            thread_ids_assigned: thread_ids,
            turn_ids_assigned: turn_ids,
        }
    }

    /// Fetch the shared process for an agent from the driver's global map.
    /// Tests use this instead of extracting it from a trait-object handle.
    fn process_for(driver: &CodexDriver, key: &AgentKey) -> Arc<CodexAgentProcess> {
        driver.ensure_process(key)
    }

    /// End-to-end multi-session: three open_session(New) calls on the same
    /// agent. Asserts distinct thread ids per session and that the events
    /// carry the correct session_id.
    #[tokio::test]
    async fn new_session_twice_returns_distinct_thread_ids() {
        let driver = CodexDriver;
        let key = "agent-multi-1".to_string();

        let s0 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let mut rx = s0.events.subscribe();

        let mut primary = s0.handle;
        let sim = install_fake_transport(&process_for(&driver, &key));

        primary.run(None).await.expect("primary run");

        let mut s1 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap()
            .handle;
        let mut s2 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap()
            .handle;

        s1.run(None).await.expect("s1 run");
        s2.run(None).await.expect("s2 run");

        let id_primary = primary.session_id().expect("primary id").to_string();
        let id1 = s1.session_id().expect("s1 id").to_string();
        let id2 = s2.session_id().expect("s2 id").to_string();

        assert_ne!(id_primary, id1, "primary and s1 must have distinct ids");
        assert_ne!(id1, id2, "s1 and s2 must have distinct ids");
        assert_ne!(id_primary, id2, "primary and s2 must have distinct ids");

        // The simulator should have handed out three distinct thread ids.
        let assigned = sim.thread_ids_assigned.lock().unwrap().clone();
        assert_eq!(
            assigned.len(),
            3,
            "exactly three thread/start responses, got {assigned:?}"
        );

        // Every session should surface a SessionAttached event on the
        // shared event stream with its own id.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let deadline = Duration::from_secs(2);
        while seen.len() < 3 {
            let ev = timeout(deadline, rx.recv())
                .await
                .expect("timed out waiting for 3 SessionAttached events")
                .expect("stream closed");
            if let DriverEvent::SessionAttached { session_id, .. } = ev {
                seen.insert(session_id);
            }
        }
        assert!(seen.contains(&id_primary));
        assert!(seen.contains(&id1));
        assert!(seen.contains(&id2));
    }

    /// `open_session(Resume)` attaches the handle to a caller-supplied thread
    /// id. Prompts on the resumed handle must flow to that same thread (the
    /// `PromptInFlight` state carries the supplied id).
    #[tokio::test]
    async fn resume_session_preserves_thread_id_on_prompt() {
        let driver = CodexDriver;
        let key = "agent-resume-1".to_string();

        let s0 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let mut primary = s0.handle;
        let _sim = install_fake_transport(&process_for(&driver, &key));
        primary.run(None).await.unwrap();

        let stored_id = "thr_stored_42".to_string();
        let mut resumed = driver
            .open_session(
                key.clone(),
                test_spec(),
                SessionIntent::Resume(stored_id.clone()),
            )
            .await
            .unwrap()
            .handle;
        resumed.run(None).await.unwrap();
        assert_eq!(
            resumed.session_id(),
            Some(stored_id.as_str()),
            "resumed handle must report the supplied thread id"
        );

        let run_id = resumed
            .prompt(PromptReq {
                text: "hi there".into(),
                attachments: vec![],
            })
            .await
            .expect("prompt succeeds on resumed session");

        // The session should flip to PromptInFlight carrying stored_id +
        // the run_id we were handed. (If the turn completes before we
        // inspect, it's back to Active but still bound to stored_id.)
        match resumed.state() {
            AgentState::PromptInFlight {
                run_id: rid,
                session_id,
            } => {
                assert_eq!(rid, run_id);
                assert_eq!(session_id, stored_id);
            }
            AgentState::Active { session_id } => {
                assert_eq!(session_id, stored_id);
            }
            other => panic!("expected PromptInFlight or Active, got {other:?}"),
        }
    }

    /// Sanity: multiple `open_session(New)` calls do NOT spawn additional
    /// transports. One child, many threads.
    #[tokio::test]
    async fn new_session_reuses_child_process() {
        let driver = CodexDriver;
        let key = "agent-shared-1".to_string();

        let s0 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let mut primary = s0.handle;
        let sim = install_fake_transport(&process_for(&driver, &key));
        primary.run(None).await.unwrap();

        let proc = driver.ensure_process(&key);
        assert!(proc.spawned.load(Ordering::SeqCst));

        for _ in 0..3 {
            let mut h = driver
                .open_session(key.clone(), test_spec(), SessionIntent::New)
                .await
                .unwrap()
                .handle;
            h.run(None).await.unwrap();
        }

        // Four thread/start responses went through ONE transport — proved
        // by the fact we only ever wired one transport (the fake's `child`
        // slot is Some).
        let ids = sim.thread_ids_assigned.lock().unwrap().clone();
        assert_eq!(
            ids.len(),
            4,
            "four thread/start requests on one shared transport"
        );
        let child_guard = proc.child.lock().unwrap();
        assert!(
            child_guard.is_some(),
            "exactly one transport owned by shared process"
        );
    }

    /// Regression: open_session → close → re-open on the same key must return
    /// a fresh `CodexAgentProcess`, not the dead cached one. Guards against
    /// the "registry never pruned" bug where close left the old Arc in the
    /// global map and the next open_session wrote into a closed stdin channel.
    #[tokio::test]
    async fn attach_close_reattach_spawns_fresh_process() {
        let driver = CodexDriver;
        let key = "agent-reattach-1".to_string();

        // --- round 1: open, run, close ---
        let s1 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let mut h1 = s1.handle;
        let _sim1 = install_fake_transport(&process_for(&driver, &key));
        h1.run(None).await.unwrap();

        // Snapshot the Arc identity so we can prove the post-reattach Arc is
        // a different allocation.
        let proc_v1_addr = {
            let guard = agent_instances().lock();
            let proc = guard.get(&key).expect("entry present after attach");
            Arc::as_ptr(proc) as usize
        };

        h1.close().await.expect("close succeeds");

        // Registry entry must have been removed — otherwise `ensure_process`
        // on re-open would hand back the dead Arc.
        assert!(
            agent_instances().get(&key).is_none(),
            "close on the last live session must prune the agent from the registry"
        );

        // --- round 2: re-open on the same key ---
        let s2 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let mut h2 = s2.handle;

        let proc_v2 = process_for(&driver, &key);
        let proc_v2_addr = Arc::as_ptr(&proc_v2) as usize;
        assert_ne!(
            proc_v1_addr, proc_v2_addr,
            "re-open must build a fresh CodexAgentProcess, not recycle the stale one"
        );
        assert!(
            !proc_v2.spawned.load(Ordering::SeqCst),
            "fresh process must have spawned=false so ensure_process_started wires a new transport"
        );

        // Wire a new transport on the fresh process and drive the
        // thread/start round-trip. Proves the new handle is functionally
        // live, not just structurally fresh.
        let _sim2 = install_fake_transport(&proc_v2);
        h2.run(None)
            .await
            .expect("run on re-opened handle must succeed via fresh transport");
        assert!(
            h2.session_id().is_some(),
            "re-opened handle must obtain a thread_id"
        );
    }

    // -----------------------------------------------------------------------
    // open_session / run behavioral tests
    // -----------------------------------------------------------------------

    /// `open_session(New)` → `session_id()` is `None` before `run()`.
    /// The codex driver sets no pre-assigned id for `New` intent, so the
    /// handle should report `None` until `run_inner` completes the
    /// `thread/start` RPC.
    #[tokio::test]
    async fn open_session_new_session_id_none_before_run() {
        let driver = CodexDriver;
        let key = format!("codex-os-new-{}", uuid::Uuid::new_v4());
        agent_instances().remove(&key);

        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();

        // Before run(): session_id() must be None (no preassigned id).
        assert_eq!(
            result.handle.session_id(),
            None,
            "open_session(New): session_id() must be None before run()"
        );

        agent_instances().remove(&key);
    }

    /// `open_session(Resume("thr_resume_xyz"))` → `session_id()` returns
    /// `Some("thr_resume_xyz")` before `run()`. After `run()` the simulator
    /// echoes back that same id (via `thread/resume`), so `session_id()` still
    /// returns `Some("thr_resume_xyz")` and the simulator's recorded thread ids
    /// confirm a `thread/resume` was sent (not `thread/start`).
    #[tokio::test]
    async fn open_session_resume_session_id_before_and_after_run() {
        let driver = CodexDriver;
        let key = format!("codex-os-resume-{}", uuid::Uuid::new_v4());
        agent_instances().remove(&key);

        let resume_id = "thr_resume_xyz".to_string();

        let result = driver
            .open_session(
                key.clone(),
                test_spec(),
                SessionIntent::Resume(resume_id.clone()),
            )
            .await
            .unwrap();

        // Before run(): session_id() must return Some(resume_id) because
        // open_session(Resume) sets resume_session_id.
        assert_eq!(
            result.handle.session_id(),
            Some(resume_id.as_str()),
            "open_session(Resume): session_id() must return Some(id) before run()"
        );

        // Wire a fake transport so run() doesn't need a real codex binary.
        let proc = process_for(&driver, &key);
        let sim = install_fake_transport(&proc);

        let mut handle = result.handle;
        handle.run(None).await.unwrap();

        // After run(): session_id() must still return the resumed id (the
        // simulator echoes it back verbatim for thread/resume).
        assert_eq!(
            handle.session_id(),
            Some(resume_id.as_str()),
            "open_session(Resume): session_id() must still return Some(id) after run()"
        );

        // The simulator must have recorded a thread/resume (not thread/start)
        // carrying our resume_id.
        {
            let assigned = sim.thread_ids_assigned.lock().unwrap().clone();
            assert_eq!(
                assigned.len(),
                1,
                "run() must produce exactly one thread response, got {assigned:?}"
            );
            assert_eq!(
                assigned[0], resume_id,
                "thread/resume must carry the supplied resume_id, got: {:?}",
                assigned[0]
            );
        }

        handle.close().await.unwrap();
        agent_instances().remove(&key);
    }
}
