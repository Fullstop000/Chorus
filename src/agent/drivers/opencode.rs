//! Native v2 driver for the OpenCode runtime using ACP protocol.
//!
//! # Multi-session architecture (Phase 0.9 Stage 2)
//!
//! A single `opencode acp` child process multiplexes several ACP sessions.
//! We model this with a shared `OpencodeAgentProcess` per agent key:
//!
//! - The first `attach` creates the process shell; `start` spawns the child
//!   and drives the initial `initialize` + `session/new` handshake (ids 1, 2).
//! - `new_session` and `resume_session` reuse the existing child, sending a
//!   fresh `session/new` / `session/load` on the same stdin. The response is
//!   delivered back via a oneshot channel keyed by the JSON-RPC id.
//! - Every handle returned from this driver shares the process's event stream.
//!   Events carry `session_id`, so consumers can route to the owning session.
//!
//! # ID-based response routing (important)
//!
//! `acp_protocol::parse_line` classifies JSON-RPC responses by id: 1 →
//! initialize, 2 → session, ≥3 → prompt. That rule breaks once we send a
//! second `session/new` with id ≥3 — the parser would call it a prompt
//! response. This driver works around the limitation *locally*, without
//! touching the shared protocol parser. We keep a per-process
//! `pending_requests: HashMap<u64, PendingKind>` that records what each
//! outgoing id was. The reader consults it before handing the frame off to
//! the right handler. Notifications (`session/update`,
//! `session/request_permission`) and errors are unaffected — `parse_line`
//! still does the structural work; we only override response classification.

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::acp_protocol::{self, AcpParsed, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcp.chat` config block for `opencode.json`.
///
/// Produces the remote HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Factored out so config-shape
/// tests don't need a live bridge.
fn build_mcp_chat_config(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "remote",
        "url": super::bridge_mcp_url(bridge_endpoint),
        "headers": {
            "X-Agent-Id": agent_key,
        },
    })
}

// ---------------------------------------------------------------------------
// OpencodeDriver
// ---------------------------------------------------------------------------

/// Unit-like driver; the shared per-agent process registry lives in a
/// process-global singleton (see `agent_instances()`). This keeps the
/// constructor call-site compatible with `Arc::new(OpencodeDriver)` in the
/// agent manager, while still letting `new_session` / `resume_session` reach
/// the same `OpencodeAgentProcess` the `attach` on that key created.
pub struct OpencodeDriver;

/// Timeout for ACP `session/new` and `session/load` responses from the
/// opencode child. If the runtime never answers, fail loudly rather than
/// hanging the caller.
const SESSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// `FactoryPath` is private to this module — only opencode branches on
// bootstrap-vs-secondary at the handle level.

/// Process-global registry: agent key -> shared runtime process. Populated
/// by `attach`; reused by subsequent `new_session` / `resume_session` calls
/// on the same key.
fn agent_instances() -> &'static AgentRegistry<OpencodeAgentProcess> {
    static INSTANCES: AgentRegistry<OpencodeAgentProcess> = AgentRegistry::new();
    &INSTANCES
}

impl OpencodeDriver {
    /// Return the existing shared process for `key`, or create one if it's
    /// the first `attach` for this agent. Stale-entry eviction happens
    /// inside [`AgentRegistry::get_or_init`].
    fn ensure_process(&self, key: &AgentKey) -> Arc<OpencodeAgentProcess> {
        agent_instances().get_or_init(key, || {
            let (events, event_tx) = EventFanOut::new();
            Arc::new(OpencodeAgentProcess {
                key: key.clone(),
                events,
                event_tx,
                child: Mutex::new(None),
                stdin_tx: Mutex::new(None),
                shared: Arc::new(Mutex::new(SharedReaderState::new())),
                // Starts at 3: ids 1 (initialize) and 2 (first session request)
                // are reserved by `start_bootstrap_child`. If an init prompt
                // is present, `start_bootstrap_child` immediately calls
                // `alloc_id()` to reserve id 3 for the deferred prompt before
                // any secondary `new_session` can race it.
                next_request_id: AtomicU64::new(3),
                reader_handles: Mutex::new(Vec::new()),
                started: std::sync::atomic::AtomicBool::new(false),
            })
        })
    }
}

fn parse_opencode_models(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[async_trait]
impl RuntimeDriver for OpencodeDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Opencode
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("opencode") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        let auth = run_command("opencode", &["--version"])
            .ok()
            .map(|result| {
                if result.success {
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
            reason: "opencode does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        if !command_exists("opencode") {
            return Ok(Vec::new());
        }

        let result = run_command("opencode", &["models"])?;
        if !result.success {
            bail!("opencode: failed to list models: {}", result.stderr.trim());
        }

        Ok(parse_opencode_models(&result.stdout)
            .into_iter()
            .map(ModelInfo::from_id)
            .collect())
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    /// Native unified factory. Bootstrap path (no live child yet): allocates
    /// handle only, zero wire I/O. Secondary path (live child): does eager
    /// wire I/O to mint/load the session id, but emits **no** `DriverEvent`s
    /// — that contract belongs to `run`.
    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        let proc = self.ensure_process(&key);
        if proc.started.load(Ordering::SeqCst) {
            // Secondary path: child is live. Do wire I/O in factory;
            // do NOT emit any DriverEvent here.
            let session_id = match &intent {
                SessionIntent::New => proc
                    .request_new_session(&spec)
                    .await
                    .context("opencode: session/new request failed")?,
                SessionIntent::Resume(id) => proc
                    .request_load_session(&spec, id)
                    .await
                    .context("opencode: session/load request failed")?,
            };
            let handle = OpencodeHandle {
                key,
                local_state: ProcessState::Idle,
                spec,
                proc: Arc::clone(&proc),
                preassigned_session_id: Some(session_id),
                factory_path: FactoryPath::Secondary,
            };
            Ok(SessionAttachment {
                session: Box::new(handle),
                events: proc.events.clone(),
            })
        } else {
            // Bootstrap path: allocate handle only, no wire I/O.
            let preassigned = match intent {
                SessionIntent::New => None,
                SessionIntent::Resume(id) => Some(id),
            };
            let handle = OpencodeHandle {
                key,
                local_state: ProcessState::Idle,
                spec,
                proc: Arc::clone(&proc),
                preassigned_session_id: preassigned,
                factory_path: FactoryPath::Bootstrap,
            };
            Ok(SessionAttachment {
                session: Box::new(handle),
                events: proc.events.clone(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Pending-request map entries
// ---------------------------------------------------------------------------

/// What an outgoing JSON-RPC id was. Used by the reader to route responses
/// correctly when `acp_protocol::parse_line`'s id-based classification would
/// misclassify them (any id ≥ 3 looks like a prompt response to the parser).
enum PendingKind {
    /// id 1 — the one-shot handshake initialize.
    Initialize,
    /// The inline handshake `session/new` (id 2) or any later one spawned via
    /// `new_session`. The oneshot delivers the minted session id back to the
    /// caller, or an error if the runtime failed. `None` for the bootstrap
    /// handshake — the bootstrap handle observes the session id through the
    /// fan-out event stream, not a direct oneshot.
    NewSession {
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// `session/load` for resuming a caller-supplied session id. Included
    /// here so we can echo the id back through the oneshot even when the
    /// runtime's response body omits `sessionId` (some do). `responder` is
    /// `None` for the bootstrap path (same reasoning as `NewSession`).
    LoadSession {
        requested_session_id: String,
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// A `session/prompt`. Carries enough context for the reader to emit the
    /// correct `Completed` event when the response arrives.
    Prompt { session_id: String, run_id: RunId },
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

/// Per-session live state tracked by the reader. One entry per `session_id`.
struct SessionRuntimeState {
    /// Active run-id when a prompt is in flight. Cleared on response.
    run_id: Option<RunId>,
    /// Tool-call accumulator is per-session because ids are only unique
    /// within a session; mixing sessions would merge calls incorrectly.
    accumulator: ToolCallAccumulator,
    /// Latest state this session has transitioned into. Mirrors what was
    /// emitted on the shared event stream.
    agent_state: ProcessState,
}

impl SessionRuntimeState {
    fn active(session_id: &str) -> Self {
        Self {
            run_id: None,
            accumulator: ToolCallAccumulator::new(),
            agent_state: ProcessState::Active {
                session_id: session_id.to_string(),
            },
        }
    }
}

struct SharedReaderState {
    /// Classifier for in-flight JSON-RPC responses. Consulted by the reader
    /// before interpreting a response frame.
    pending_requests: HashMap<u64, PendingKind>,
    /// Per-session live state, keyed by the runtime's `sessionId`.
    sessions: HashMap<String, SessionRuntimeState>,
    /// An initial-prompt deferred until the bootstrap `session/new` response
    /// arrives and mints the session id. Holds `(request_id, text)` where
    /// `request_id` was reserved up-front via `alloc_id()` at handshake time
    /// so it can never collide with a racing secondary `new_session`. `None`
    /// after the first `SessionResponse` consumes it.
    bootstrap_pending_prompt: Option<(u64, String)>,
    /// Caller-supplied resume id for the initial handshake (id 2). If
    /// `session/load` omits `sessionId`, we fall back to this.
    bootstrap_requested_session_id: Option<String>,
    /// Session id minted by (or loaded into) the bootstrap's id-2
    /// `session/new` | `session/load` response. The bootstrap handle
    /// doesn't locally track its session_id until after the response lands
    /// on the reader, so we stash it here. `close()` on the bootstrap uses
    /// this to drop exactly *its* slot from `sessions` without taking out
    /// a live secondary's entry.
    bootstrap_session_id: Option<String>,
}

impl SharedReaderState {
    fn new() -> Self {
        Self {
            pending_requests: HashMap::new(),
            sessions: HashMap::new(),
            bootstrap_pending_prompt: None,
            bootstrap_requested_session_id: None,
            bootstrap_session_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// OpencodeAgentProcess
// ---------------------------------------------------------------------------

/// Shared runtime process for one agent. Multiple `OpencodeHandle`s may hold
/// an `Arc` to the same process and concurrently drive distinct sessions on
/// it.
pub struct OpencodeAgentProcess {
    /// Agent key this process belongs to. Used in tracing from the reader
    /// and teardown paths so log lines tie back to the owning agent.
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    child: Mutex<Option<std::process::Child>>,
    stdin_tx: Mutex<Option<mpsc::Sender<String>>>,
    shared: Arc<Mutex<SharedReaderState>>,
    /// Next JSON-RPC request id. Starts at 3 because ids 1 (initialize)
    /// and 2 (first session request) are reserved by the handshake. If an
    /// init prompt is deferred, `start_bootstrap_child` burns id 3 off this
    /// counter up-front so a racing secondary `new_session` cannot collide.
    next_request_id: AtomicU64,
    reader_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Flipped to true once `start` has spawned the child and written the
    /// handshake. Gates `new_session` / `resume_session`.
    started: std::sync::atomic::AtomicBool,
}

impl OpencodeAgentProcess {
    fn alloc_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a raw JSON-RPC line on the shared stdin. Returns `Err` if the
    /// child is no longer live.
    async fn send_line(&self, line: String) -> anyhow::Result<()> {
        let tx = {
            let guard = self.stdin_tx.lock().unwrap();
            guard.clone()
        };
        let tx = tx.context("opencode: stdin not available — child not started")?;
        tx.send(line)
            .await
            .context("opencode: stdin channel closed")
    }

    /// Register a pending response classifier under `id`.
    fn register_pending(&self, id: u64, kind: PendingKind) {
        self.shared
            .lock()
            .unwrap()
            .pending_requests
            .insert(id, kind);
    }

    /// Send `session/new` and wait for the minted session id.
    async fn request_new_session(&self, spec: &AgentSpec) -> anyhow::Result<String> {
        let id = self.alloc_id();
        let (responder, rx) = oneshot::channel();
        self.register_pending(
            id,
            PendingKind::NewSession {
                responder: Some(responder),
            },
        );

        let params = serde_json::json!({
            "cwd": spec.working_directory,
            "mcpServers": []
        });
        let req = acp_protocol::build_session_new_request(id, params);
        self.send_line(req).await?;

        // Guard against a stuck child: if the runtime never answers, fail
        // loudly rather than hang the caller.
        let res = tokio::time::timeout(SESSION_REQUEST_TIMEOUT, rx)
            .await
            .context("opencode: timed out waiting for session/new response")?
            .context("opencode: session/new responder dropped")?;
        res
    }

    /// Send `session/load` and wait for confirmation, returning the resumed
    /// session id (falling back to the caller-supplied id if the runtime
    /// omits it in the response).
    async fn request_load_session(
        &self,
        spec: &AgentSpec,
        session_id: &str,
    ) -> anyhow::Result<String> {
        let id = self.alloc_id();
        let (responder, rx) = oneshot::channel();
        self.register_pending(
            id,
            PendingKind::LoadSession {
                requested_session_id: session_id.to_string(),
                responder: Some(responder),
            },
        );

        let params = serde_json::json!({
            "cwd": spec.working_directory,
            "mcpServers": []
        });
        let req = acp_protocol::build_session_load_request(id, session_id, params);
        self.send_line(req).await?;

        let res = tokio::time::timeout(SESSION_REQUEST_TIMEOUT, rx)
            .await
            .context("opencode: timed out waiting for session/load response")?
            .context("opencode: session/load responder dropped")?;
        res
    }

    /// Signal the child to exit. Idempotent.
    fn kill_child(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(ref mut child) = *guard {
            let pid = child.id();
            if let Err(e) = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            ) {
                warn!(key = %self.key, pid, error = %e, "opencode: failed to SIGTERM child");
            }
        }
        *guard = None;
    }
}

impl AgentProcess for OpencodeAgentProcess {
    const DRIVER_NAME: &'static str = "opencode";

    /// Returns `true` once the shared child has gone away — either the
    /// process exited (stdin writer dropped the receiver) or was never
    /// wired up. The [`AgentRegistry`] evicts stale entries so the next
    /// `attach` rebuilds from scratch.
    fn is_stale(&self) -> bool {
        if !self.started.load(Ordering::SeqCst) {
            // Never spawned — not stale, just fresh.
            return false;
        }
        let guard = self.stdin_tx.lock().unwrap();
        match guard.as_ref() {
            // Started flag flipped but wiring not landed yet — transient;
            // treat as live so we don't tear down a process mid-spawn.
            None => false,
            // Writer task exited (dropped the receiver) → child is dead.
            Some(tx) => tx.is_closed(),
        }
    }
}

impl Drop for OpencodeAgentProcess {
    fn drop(&mut self) {
        self.kill_child();
        let mut handles = self.reader_handles.lock().unwrap();
        for h in handles.drain(..) {
            h.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// OpencodeHandle
// ---------------------------------------------------------------------------

/// Which path the `open_session` factory took when constructing this handle.
///
/// `Bootstrap` — no live child yet; this handle will spawn the child and
/// drive the initialize + session/new handshake when `run()` is called.
///
/// `Secondary` — child is already live; the factory did the wire I/O to
/// mint/load a session id before returning. `run()` only needs to seed
/// per-session state and emit lifecycle events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FactoryPath {
    Bootstrap,
    Secondary,
}

impl FactoryPath {
    fn is_bootstrap(self) -> bool {
        matches!(self, Self::Bootstrap)
    }
}

pub struct OpencodeHandle {
    key: AgentKey,
    /// Local view of this handle's lifecycle. Authoritative state for a
    /// session lives in `proc.shared.sessions[session_id]`; this mirror is
    /// used for synchronous read methods (`session_id`, `state`) without
    /// taking the shared lock.
    local_state: ProcessState,
    spec: AgentSpec,
    proc: Arc<OpencodeAgentProcess>,
    /// Set by `new_session` / `resume_session` so `start` knows this handle
    /// is attaching to an already-minted session id on the shared child.
    preassigned_session_id: Option<SessionId>,
    /// Which factory path created this handle. `Bootstrap` owns process
    /// spawn + handshake; `Secondary` joins an already-live child.
    factory_path: FactoryPath,
}

impl OpencodeHandle {
    fn emit(&self, event: DriverEvent) {
        super::emit_driver_event(
            &self.proc.event_tx,
            event,
            &self.key,
            <OpencodeAgentProcess as AgentProcess>::DRIVER_NAME,
        );
    }
}

#[async_trait]
impl Session for OpencodeHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.local_state {
            ProcessState::Active { session_id } => Some(session_id),
            ProcessState::PromptInFlight { session_id, .. } => Some(session_id),
            _ => self.preassigned_session_id.as_deref(),
        }
    }

    fn process_state(&self) -> ProcessState {
        let shared_session_id = match &self.local_state {
            ProcessState::Active { session_id } => Some(session_id.as_str()),
            ProcessState::PromptInFlight { session_id, .. } => Some(session_id.as_str()),
            _ => self.preassigned_session_id.as_deref(),
        };

        if let Some(session_id) = shared_session_id {
            if let Ok(shared) = self.proc.shared.lock() {
                if let Some(session) = shared.sessions.get(session_id) {
                    return session.agent_state.clone();
                }
            }
        }

        self.local_state.clone()
    }

    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        match self.factory_path {
            FactoryPath::Bootstrap => self.run_bootstrap(init_prompt).await,
            FactoryPath::Secondary => self.run_secondary(init_prompt).await,
        }
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = match &self.local_state {
            ProcessState::Active { session_id } => session_id.clone(),
            _ => bail!("cannot prompt: handle not in Active state"),
        };

        let run_id = RunId::new_v4();
        let request_id = self.proc.alloc_id();

        // Record pending classifier + per-session run tracking in one place.
        {
            let mut s = self.proc.shared.lock().unwrap();
            s.pending_requests.insert(
                request_id,
                PendingKind::Prompt {
                    session_id: session_id.clone(),
                    run_id,
                },
            );
            if let Some(sess) = s.sessions.get_mut(&session_id) {
                sess.run_id = Some(run_id);
                sess.agent_state = ProcessState::PromptInFlight {
                    run_id,
                    session_id: session_id.clone(),
                };
            }
        }

        self.local_state = ProcessState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let prompt_req =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);
        self.proc.send_line(prompt_req).await?;

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        let cancel_info = match &self.local_state {
            ProcessState::PromptInFlight { run_id, session_id } => {
                Some((*run_id, session_id.clone()))
            }
            _ => None,
        };
        if let Some((run_id, session_id)) = cancel_info {
            {
                let mut s = self.proc.shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&session_id) {
                    sess.run_id = None;
                    sess.agent_state = ProcessState::Active {
                        session_id: session_id.clone(),
                    };
                }
            }

            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Cancelled,
                },
            });

            self.local_state = ProcessState::Active { session_id };
            Ok(CancelOutcome::Aborted)
        } else {
            Ok(CancelOutcome::NotInFlight)
        }
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.local_state, ProcessState::Closed) {
            return Ok(());
        }

        // Drop this handle's session slot from shared state and, under the
        // same lock, determine whether any session is still live on the
        // shared child.
        //
        // Bootstrap subtlety: the bootstrap handle's `session_id()` returns
        // `None` until the id-2 `session/new` response lands on the reader
        // and transitions `shared.sessions`. The reader records that minted
        // id in `shared.bootstrap_session_id` specifically so a bootstrap
        // close that happens *after* warmup but *before* any prompt can
        // still locate its own slot.
        let all_sessions_closed = {
            let mut s = self.proc.shared.lock().unwrap();
            let sid_to_remove: Option<String> = if self.factory_path.is_bootstrap() {
                self.session_id()
                    .map(|s| s.to_string())
                    .or_else(|| s.bootstrap_session_id.clone())
            } else {
                self.session_id().map(|s| s.to_string())
            };
            if let Some(ref sid) = sid_to_remove {
                s.sessions.remove(sid);
            }
            // Don't tear down while a session/new or session/load response
            // is pending — the caller is awaiting a session that would lose
            // its backing child if we killed it now. Mirrors codex's guard.
            let no_pending_session_creation = !s.pending_requests.values().any(|p| {
                matches!(
                    p,
                    PendingKind::NewSession { .. } | PendingKind::LoadSession { .. }
                )
            });
            s.sessions.is_empty() && no_pending_session_creation
        };

        self.local_state = ProcessState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: ProcessState::Closed,
        });

        // Teardown of the shared child + fan-out + registry is gated on
        // *all sessions closed*, regardless of role. A bootstrap close with
        // a live secondary (still Active or mid-prompt) must NOT kill the
        // child — the secondary would lose its stdin, its reader task would
        // be aborted mid-stream, and no terminal event would reach the
        // caller. The last session to close (either role) triggers teardown
        // here.
        if all_sessions_closed {
            self.proc.kill_child();
            self.proc.events.close();
            {
                let mut handles = self.proc.reader_handles.lock().unwrap();
                for h in handles.drain(..) {
                    h.abort();
                }
            }
            // Evict from the process-global registry so a subsequent
            // `attach` on this key doesn't reuse our now-dead Arc (with
            // killed child + closed stdin). `ensure_process`'s `is_stale`
            // check would catch it anyway, but dropping the map entry also
            // lets the `OpencodeAgentProcess` ref drop when the last handle
            // releases it.
            agent_instances().remove(&self.key);
        }

        Ok(())
    }
}

impl OpencodeHandle {
    /// Bootstrap run path: emit Starting lifecycle, spawn the child, write the
    /// handshake, and set up the reader tasks. Reads `self.preassigned_session_id`
    /// for resume (set by `start` shim from `opts.resume_session_id`, or by
    /// `open_session(Resume(id))` directly).
    async fn run_bootstrap(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.local_state = ProcessState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: ProcessState::Starting,
        });
        self.run_bootstrap_inner(init_prompt).await
    }

    /// Secondary run path: child is already live, session id was minted by
    /// `open_session`. Emit Starting, seed per-session state, emit
    /// SessionAttached + Active, then deliver init_prompt if present.
    async fn run_secondary(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.local_state = ProcessState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: ProcessState::Starting,
        });

        // `preassigned_session_id` was set by `open_session` (Secondary path
        // does eager wire I/O in the factory).
        let session_id = self
            .preassigned_session_id
            .clone()
            .context("opencode: handle spawned without preassigned session id")?;

        // Seed per-session runtime state and announce the attach.
        {
            let mut s = self.proc.shared.lock().unwrap();
            s.sessions
                .entry(session_id.clone())
                .or_insert_with(|| SessionRuntimeState::active(&session_id));
        }
        self.local_state = ProcessState::Active {
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::SessionAttached {
            key: self.key.clone(),
            session_id: session_id.clone(),
        });
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: ProcessState::Active {
                session_id: session_id.clone(),
            },
        });

        if let Some(req) = init_prompt {
            self.prompt(req).await?;
        }

        Ok(())
    }

    /// Inner bootstrap implementation: spawn the child, write the handshake,
    /// and set up the reader tasks. Only called from `run_bootstrap`.
    async fn run_bootstrap_inner(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        // Read resume id from preassigned_session_id (set by start shim or open_session).
        let resume_session_id = self.preassigned_session_id.clone();

        let wd = &self.spec.working_directory;
        let model_id = match &self.spec.reasoning_effort {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}", self.spec.model, variant)
            }
            _ => self.spec.model.clone(),
        };

        let endpoint = &self.spec.bridge_endpoint;

        // Write opencode.json to the working directory.
        let config_path = wd.join("opencode.json");
        let mcp_chat = build_mcp_chat_config(endpoint, &self.key);
        let opencode_config = serde_json::json!({
            "model": model_id,
            "mcp": {
                "chat": mcp_chat,
            }
        });
        tokio::fs::write(
            &config_path,
            serde_json::to_string_pretty(&opencode_config)?,
        )
        .await
        .context("failed to write opencode.json")?;

        let args = vec!["acp".to_string()];

        let mut cmd = Command::new("opencode");
        cmd.args(&args)
            .current_dir(&self.spec.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn opencode")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        // Reserve the deferred-prompt id BEFORE anyone else can alloc. This
        // prevents a race where a secondary `new_session` fires on another
        // task between us writing `initialize` (id 1) + `session/new` (id 2)
        // and the bootstrap reader landing on the session response: without
        // an up-front reservation, `alloc_id()` on that racing call would
        // return 3, collide with the hardcoded deferred-prompt id, and
        // mis-route the response. Allocating via `alloc_id()` here burns id
        // 3 off the counter even if we don't end up with an init prompt —
        // that's fine; ids are cheap and a missing id in the sequence is
        // harmless to the runtime.
        let deferred_prompt_id = if init_prompt.is_some() {
            Some(self.proc.alloc_id())
        } else {
            None
        };

        // Register handshake ids BEFORE writing, so an unexpectedly fast
        // runtime can't land a response before the pending map sees it.
        {
            let mut s = self.proc.shared.lock().unwrap();
            s.pending_requests.insert(1, PendingKind::Initialize);
            // Bootstrap session response carries `responder: None` — the
            // bootstrap handle observes the minted session id through the
            // emitted `SessionAttached` event on the shared fan-out, not a
            // direct oneshot.
            if let Some(ref sid) = resume_session_id {
                s.bootstrap_requested_session_id = Some(sid.clone());
                s.pending_requests.insert(
                    2,
                    PendingKind::LoadSession {
                        requested_session_id: sid.clone(),
                        responder: None,
                    },
                );
            } else {
                s.pending_requests
                    .insert(2, PendingKind::NewSession { responder: None });
            }
            // Stash the init prompt + its reserved id so the reader can
            // fire it once the session is minted without colliding with a
            // racing `alloc_id()` on a secondary `new_session`.
            if let (Some(ref req), Some(pid)) = (&init_prompt, deferred_prompt_id) {
                s.bootstrap_pending_prompt = Some((pid, req.text.clone()));
            }
        }

        // Write handshake synchronously before handing stdin to the async writer.
        //
        // If either write fails (child exited or closed stdin between spawn
        // and this point), we've already installed pending-request entries
        // for ids 1 and 2 (and possibly a bootstrap_requested_session_id +
        // bootstrap_pending_prompt) above. Leaving that state in the shared
        // registry would poison a subsequent `attach` that reuses the cached
        // `Arc<OpencodeAgentProcess>` — `is_stale()` returns `false` because
        // `started` hasn't been flipped yet. Roll back the partial state on
        // any handshake-write error so the registry entry doesn't carry
        // orphaned pending ids.
        let init_req = acp_protocol::build_initialize_request(1);
        let session_new_params = serde_json::json!({
            "cwd": self.spec.working_directory,
            "mcpServers": []
        });
        let session_req = if let Some(ref sid) = resume_session_id {
            acp_protocol::build_session_load_request(2, sid, session_new_params)
        } else {
            acp_protocol::build_session_new_request(2, session_new_params)
        };

        let write_result = (|| -> anyhow::Result<()> {
            writeln!(stdin, "{init_req}").context("failed to write initialize request")?;
            writeln!(stdin, "{session_req}").context("failed to write session request")?;
            Ok(())
        })();

        if let Err(e) = write_result {
            // Roll back every piece of shared state we installed above.
            // Order mirrors the install block so it's easy to audit.
            let mut s = self.proc.shared.lock().unwrap();
            s.pending_requests.remove(&1);
            s.pending_requests.remove(&2);
            s.bootstrap_requested_session_id = None;
            s.bootstrap_pending_prompt = None;
            if let Some(pid) = deferred_prompt_id {
                // `deferred_prompt_id` was only reserved off `alloc_id()` —
                // it isn't in `pending_requests` yet (that happens in the
                // reader's NewSessionResponse branch once the session id
                // lands). Defensive remove in case that ever changes.
                s.pending_requests.remove(&pid);
            }
            return Err(e);
        }

        // Stdin writer task. Plumbed through `proc.stdin_tx` so subsequent
        // sessions on this process can write too.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        {
            let mut guard = self.proc.stdin_tx.lock().unwrap();
            *guard = Some(stdin_tx.clone());
        }
        let stdin_handle = tokio::task::spawn_blocking(move || {
            while let Some(line) = stdin_rx.blocking_recv() {
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                if stdin.flush().is_err() {
                    break;
                }
            }
        });
        self.proc.reader_handles.lock().unwrap().push(stdin_handle);

        // Stdout reader task.
        let key = self.key.clone();
        let event_tx = self.proc.event_tx.clone();
        let shared = self.proc.shared.clone();
        let stdin_tx_for_reader = stdin_tx.clone();
        let stdout_handle = tokio::spawn(async move {
            let stdout_async = match tokio::process::ChildStdout::from_std(stdout) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key, error = %e, "opencode: failed to convert stdout to async");
                    return;
                }
            };
            let reader = BufReader::new(stdout_async);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                trace!(line = %line, "opencode stdout");

                // Pre-classify responses by id via the pending map, so
                // session/new responses with id ≥ 3 don't get misrouted as
                // prompt responses by the shared parser.
                let classified = classify_line(&line, &shared);

                dispatch_line(classified, &key, &event_tx, &shared, &stdin_tx_for_reader).await;
            }

            // EOF — runtime exited. Flush every session that had an
            // in-flight run.
            let pending_completions: Vec<(String, RunId)> = {
                let s = shared.lock().unwrap();
                s.sessions
                    .iter()
                    .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
                    .collect()
            };
            for (sid, run_id) in pending_completions {
                let _ = event_tx.try_send(DriverEvent::Completed {
                    key: key.clone(),
                    session_id: sid,
                    run_id,
                    result: RunResult {
                        finish_reason: FinishReason::TransportClosed,
                    },
                });
            }
            // Clear per-session state and emit a single Closed lifecycle.
            {
                let mut s = shared.lock().unwrap();
                s.sessions.clear();
            }
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: ProcessState::Closed,
            });
        });
        self.proc.reader_handles.lock().unwrap().push(stdout_handle);

        // Stderr reader task.
        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let stderr_async = match tokio::process::ChildStderr::from_std(stderr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key_err, error = %e, "opencode: failed to convert stderr to async");
                    return;
                }
            };
            let reader = BufReader::new(stderr_async);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, line = %line, "opencode stderr");
                }
            }
        });
        self.proc.reader_handles.lock().unwrap().push(stderr_handle);

        {
            let mut guard = self.proc.child.lock().unwrap();
            *guard = Some(child);
        }
        self.proc.started.store(true, Ordering::SeqCst);

        // Defer local_state transition to `Active` until the reader observes
        // the session response. Callers who need the session id block on
        // SessionAttached events through the event stream.
        if let Some(ref sid) = resume_session_id {
            // Pre-populate local mirror optimistically; the reader will
            // confirm by emitting SessionAttached / Active.
            self.local_state = ProcessState::Active {
                session_id: sid.clone(),
            };
        }
        // For fresh new_session we stay in Starting until the reader fires
        // SessionAttached. The bootstrap pending prompt (text + reserved
        // id) was stashed above on `shared.bootstrap_pending_prompt`; the
        // reader will pull it once the session response arrives.

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader dispatch — classification and handling
// ---------------------------------------------------------------------------

/// Classified event derived from one line. Distinct from `AcpParsed` so we
/// can override id-based response routing without touching `parse_line`.
enum ClassifiedFrame {
    /// id 1 initialize response. Parsed the same as `AcpParsed::InitializeResponse`.
    Initialize,
    /// `session/new` response. `session_id` is whatever the runtime returned.
    /// `responder` delivers it back to the waiting `new_session` call.
    NewSessionResponse {
        session_id: Option<String>,
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// `session/load` response.
    LoadSessionResponse {
        session_id: Option<String>,
        requested_session_id: String,
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// A prompt completed. Carries the routing context we stashed when we
    /// sent the request so we can emit the right `Completed` event.
    PromptResponse { session_id: String, run_id: RunId },
    /// Error tied to a known pending id, with context to build the
    /// correct Failed event.
    PendingError { kind: PendingKind, message: String },
    /// A notification (session/update, session/request_permission) or
    /// something unrecognized. Reused as-is from the parser. Untracked
    /// errors (response with an id not in `pending_requests`) surface here
    /// as `AcpParsed::Error` via the fallback in `classify_line`.
    PassThrough(AcpParsed),
}

/// Strip the pending classifier for this line's id if any, then turn the
/// line into a `ClassifiedFrame`. For non-response frames we delegate to
/// `acp_protocol::parse_line`.
fn classify_line(line: &str, shared: &Arc<Mutex<SharedReaderState>>) -> ClassifiedFrame {
    // Peek at the raw JSON to see if it's a response we need to reclassify.
    let raw: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return ClassifiedFrame::PassThrough(AcpParsed::Unknown),
    };

    let is_response =
        raw.get("id").is_some() && (raw.get("result").is_some() || raw.get("error").is_some());
    if !is_response {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    }

    let Some(id) = raw.get("id").and_then(|v| v.as_u64()) else {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    };

    // Extract the pending classifier. If missing, fall through to the raw
    // parser — unsolicited responses are a protocol violation and we'd
    // rather see them as Unknown than silently drop them.
    let pending = shared.lock().unwrap().pending_requests.remove(&id);
    let Some(kind) = pending else {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    };

    // Handle errors first so we can forward them to the waiting responder.
    if let Some(err) = raw.get("error") {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown ACP error")
            .to_string();
        return ClassifiedFrame::PendingError { kind, message };
    }

    let session_id = raw
        .get("result")
        .and_then(|r| r.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match kind {
        PendingKind::Initialize => ClassifiedFrame::Initialize,
        PendingKind::NewSession { responder } => ClassifiedFrame::NewSessionResponse {
            session_id,
            responder,
        },
        PendingKind::LoadSession {
            requested_session_id,
            responder,
        } => ClassifiedFrame::LoadSessionResponse {
            session_id,
            requested_session_id,
            responder,
        },
        PendingKind::Prompt {
            session_id: s,
            run_id,
        } => ClassifiedFrame::PromptResponse {
            session_id: s,
            run_id,
        },
    }
}

fn send_deferred_bootstrap_prompt(
    key: &AgentKey,
    session_id: &str,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
) {
    let Some((prompt_id, prompt_text, run_id)) = ({
        let mut s = shared.lock().unwrap();
        s.bootstrap_pending_prompt
            .take()
            .map(|(prompt_id, prompt_text)| {
                let run_id = RunId::new_v4();
                if let Some(sess) = s.sessions.get_mut(session_id) {
                    sess.run_id = Some(run_id);
                    sess.agent_state = ProcessState::PromptInFlight {
                        run_id,
                        session_id: session_id.to_string(),
                    };
                }
                s.pending_requests.insert(
                    prompt_id,
                    PendingKind::Prompt {
                        session_id: session_id.to_string(),
                        run_id,
                    },
                );
                (prompt_id, prompt_text, run_id)
            })
    }) else {
        return;
    };

    let _ = event_tx.try_send(DriverEvent::Lifecycle {
        key: key.clone(),
        state: ProcessState::PromptInFlight {
            run_id,
            session_id: session_id.to_string(),
        },
    });

    let req = acp_protocol::build_session_prompt_request(prompt_id, session_id, &prompt_text);
    let _ = stdin_tx.try_send(req);
}

/// Handle a classified line: emit events, respond on oneshots, mutate state.
async fn dispatch_line(
    frame: ClassifiedFrame,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
) {
    match frame {
        ClassifiedFrame::Initialize => {
            debug!("opencode: initialize response received");
        }

        ClassifiedFrame::NewSessionResponse {
            session_id,
            responder,
        } => {
            // Bootstrap path: `responder` is `None` — the bootstrap handle
            // observes the session id through the emitted SessionAttached
            // event on the shared fan-out.
            // new_session path: `responder` is `Some(tx)` and we hand the
            // minted id back to the caller.
            //
            // ACP spec: `session/new` MUST return a `sessionId`. If the
            // runtime omits it, that's a protocol violation — surface it
            // loudly instead of synthesizing a fake id. Synthesizing would
            // seed a `SessionRuntimeState` for an id the runtime doesn't
            // know about; any follow-up prompt/resume on that id would
            // silently fail downstream.
            let sid = match session_id {
                Some(s) => s,
                None => {
                    warn!("opencode: session/new response omitted sessionId (spec violation)");
                    match responder {
                        Some(tx) => {
                            let _ = tx.send(Err(anyhow!(
                                "opencode session/new response omitted sessionId (protocol violation)"
                            )));
                        }
                        None => {
                            // Bootstrap path: there is no caller-side oneshot
                            // to route the error to. Emit Failed on the shared
                            // fan-out so the forwarder surfaces the violation.
                            // Use a nil RunId because no run was in flight.
                            let _ = event_tx.try_send(DriverEvent::Failed {
                                key: key.clone(),
                                session_id: String::new(),
                                run_id: uuid::Uuid::nil(),
                                error: AgentError::Protocol(
                                    "opencode session/new response omitted sessionId".into(),
                                ),
                            });
                        }
                    }
                    return;
                }
            };

            // Seed per-session state.
            {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(sid.clone())
                    .or_insert_with(|| SessionRuntimeState::active(&sid));
                // `responder.is_none()` identifies the bootstrap path —
                // secondary `new_session` calls always supply a responder.
                // Record the bootstrap's minted session id so its `close()`
                // can drop exactly that slot without taking out live
                // secondaries.
                if responder.is_none() && s.bootstrap_session_id.is_none() {
                    s.bootstrap_session_id = Some(sid.clone());
                }
            }

            let is_bootstrap = responder.is_none();
            if let Some(responder) = responder {
                if responder.send(Ok(sid.clone())).is_err() {
                    // Caller dropped. That's okay — we already seeded state.
                }
            }

            // Only the bootstrap path announces the attach here. For the
            // secondary path (`responder.is_some()`), the secondary handle's
            // `start()` emits SessionAttached + Active itself — emitting here
            // too would double-fire and desync `await_session_attached`-style
            // consumers. Mirrors kimi's split: reader emits for warmup/
            // bootstrap; handle.start() emits for secondary.
            if is_bootstrap {
                let _ = event_tx.try_send(DriverEvent::SessionAttached {
                    key: key.clone(),
                    session_id: sid.clone(),
                });
                let _ = event_tx.try_send(DriverEvent::Lifecycle {
                    key: key.clone(),
                    state: ProcessState::Active {
                        session_id: sid.clone(),
                    },
                });
                send_deferred_bootstrap_prompt(key, &sid, event_tx, shared, stdin_tx);
            }
        }

        ClassifiedFrame::LoadSessionResponse {
            session_id,
            requested_session_id,
            responder,
        } => {
            let sid = session_id.unwrap_or(requested_session_id);
            {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(sid.clone())
                    .or_insert_with(|| SessionRuntimeState::active(&sid));
                s.bootstrap_requested_session_id = None;
                // `responder.is_none()` identifies the bootstrap path
                // (secondary `resume_session` supplies a responder). Record
                // the bootstrap's session id for its `close()` teardown.
                if responder.is_none() && s.bootstrap_session_id.is_none() {
                    s.bootstrap_session_id = Some(sid.clone());
                }
            }
            let is_bootstrap = responder.is_none();
            if let Some(responder) = responder {
                let _ = responder.send(Ok(sid.clone()));
            }
            // Only the bootstrap path announces the attach here. For the
            // secondary resume path (`responder.is_some()`), the secondary
            // handle's `start()` emits SessionAttached + Active itself.
            // Mirrors kimi's split: reader emits for warmup/bootstrap;
            // handle.start() emits for secondary.
            if is_bootstrap {
                let _ = event_tx.try_send(DriverEvent::SessionAttached {
                    key: key.clone(),
                    session_id: sid.clone(),
                });
                let _ = event_tx.try_send(DriverEvent::Lifecycle {
                    key: key.clone(),
                    state: ProcessState::Active {
                        session_id: sid.clone(),
                    },
                });
                send_deferred_bootstrap_prompt(key, &sid, event_tx, shared, stdin_tx);
            }
        }

        ClassifiedFrame::PromptResponse { session_id, run_id } => {
            // Flush any pending tool-call accumulator on the matching session,
            // then emit TurnEnd + Completed.
            let drained: Vec<(Option<String>, String, serde_json::Value)> = {
                let mut s = shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&session_id) {
                    sess.run_id = None;
                    sess.agent_state = ProcessState::Active {
                        session_id: session_id.clone(),
                    };
                    sess.accumulator.drain()
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
                state: ProcessState::Active { session_id },
            });
        }

        ClassifiedFrame::PendingError { kind, message } => {
            warn!(message = %message, "opencode: ACP error");
            match kind {
                PendingKind::Initialize => {
                    // Initialize failing is terminal. The EOF path normally
                    // emits `Closed`, but a runtime that replies with a
                    // JSON-RPC error and keeps stdin open would leave the
                    // process zombied with `started=true` and no Closed
                    // lifecycle ever fired. Force the teardown here so
                    // downstream consumers observe the failure.
                    let _ = event_tx.try_send(DriverEvent::Lifecycle {
                        key: key.clone(),
                        state: ProcessState::Closed,
                    });
                    // Clear per-session state so a subsequent attach on this
                    // key can proceed cleanly; the `is_stale` check in
                    // `ensure_process` will evict the registry entry once
                    // the child actually exits (or the user closes the
                    // bootstrap handle).
                    let mut s = shared.lock().unwrap();
                    s.sessions.clear();
                }
                PendingKind::NewSession { responder } => {
                    if let Some(tx) = responder {
                        let _ = tx.send(Err(anyhow::anyhow!("{message}")));
                    }
                }
                PendingKind::LoadSession { responder, .. } => {
                    if let Some(tx) = responder {
                        let _ = tx.send(Err(anyhow::anyhow!("{message}")));
                    }
                }
                PendingKind::Prompt { session_id, run_id } => {
                    {
                        let mut s = shared.lock().unwrap();
                        if let Some(sess) = s.sessions.get_mut(&session_id) {
                            sess.run_id = None;
                            sess.agent_state = ProcessState::Active {
                                session_id: session_id.clone(),
                            };
                        }
                    }
                    let _ = event_tx.try_send(DriverEvent::Failed {
                        key: key.clone(),
                        session_id,
                        run_id,
                        error: AgentError::RuntimeReported(message),
                    });
                }
            }
        }

        ClassifiedFrame::PassThrough(parsed) => match parsed {
            AcpParsed::InitializeResponse => {
                debug!("opencode: initialize response (untracked)");
            }
            AcpParsed::SessionResponse { .. } | AcpParsed::PromptResponse { .. } => {
                // Shouldn't happen: responses always go through classify_line's
                // pending-map path. Log so drift is visible.
                warn!("opencode: response not matched by pending_requests; dropped");
            }
            AcpParsed::SessionUpdate { items } => {
                handle_session_update(items, key, event_tx, shared).await;
            }
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                let option_id = acp_protocol::pick_best_option_id(&options);
                debug!(
                    ?tool_name,
                    request_id, option_id, "opencode: auto-approving permission"
                );
                let response = acp_protocol::build_permission_response_raw(request_id, option_id);
                let _ = stdin_tx.try_send(response);
            }
            AcpParsed::Error { message } => {
                warn!(message = %message, "opencode: ACP error (untracked)");
            }
            AcpParsed::Unknown => {}
        },
    }
}

/// Apply the items from a `session/update` notification to the correct
/// per-session accumulator + event stream. We prefer the runtime-provided
/// `SessionInit` session id when present; otherwise any session that's in
/// PromptInFlight state is a candidate — this mirrors how v1 fell back on a
/// single known session id in the single-session case.
async fn handle_session_update(
    items: Vec<AcpUpdateItem>,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
) {
    // Determine the target session id for these items. `session/update`
    // frames should carry a `sessionId` that `acp_protocol::parse_line`
    // prepends as `SessionInit { session_id }` at `items[0]`. If that is
    // missing the frame is either malformed or from an older spec — we
    // fall back to routing heuristics below.
    //
    // Fallback policy: safe only when there is at most one in-flight
    // session. With 2+ sessions in flight and no SessionInit, we cannot
    // attribute the update without guessing — HashMap iteration order is
    // nondeterministic, so a guess silently cross-contaminates runs. In
    // that case we drop the update with a loud warn and let higher layers
    // notice the gap rather than mis-route.
    let (target_session_id, run_id_opt): (Option<String>, Option<RunId>) = {
        let s = shared.lock().unwrap();
        // Pull any SessionInit item first — if present, it's authoritative.
        let init_sid = items.iter().find_map(|it| match it {
            AcpUpdateItem::SessionInit { session_id } => Some(session_id.clone()),
            _ => None,
        });
        if let Some(sid) = init_sid {
            let run_id = s.sessions.get(&sid).and_then(|st| st.run_id);
            (Some(sid), run_id)
        } else {
            let in_flight: Vec<(String, RunId)> = s
                .sessions
                .iter()
                .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
                .collect();
            match in_flight.len() {
                0 => (None, None),
                1 => {
                    let (sid, run_id) = in_flight.into_iter().next().unwrap();
                    (Some(sid), Some(run_id))
                }
                n => {
                    warn!(
                        agent = %key,
                        in_flight = n,
                        "opencode: session/update missing SessionInit with multiple \
                         in-flight sessions — dropping update (cannot attribute safely)"
                    );
                    (None, None)
                }
            }
        }
    };

    let (Some(sid), Some(run_id)) = (target_session_id, run_id_opt) else {
        return;
    };

    // Drive per-session accumulator and emissions.
    let mut drained_tool_calls: Vec<(Option<String>, String, serde_json::Value)> = Vec::new();
    for item in items {
        match item {
            AcpUpdateItem::SessionInit { .. } => {
                // Already used above.
            }
            AcpUpdateItem::Thinking { text } => {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::Thinking { text },
                });
            }
            AcpUpdateItem::Text { text } => {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::Text { text },
                });
            }
            AcpUpdateItem::ToolCall { id, name, input } => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    let acc = s
                        .sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default();
                    if let Some(sess) = s.sessions.get_mut(&sid) {
                        sess.accumulator.record_call(id, name, input);
                    }
                    acc
                };
                drained_tool_calls.extend(pending_before);
            }
            AcpUpdateItem::ToolCallUpdate { id, input } => {
                let mut s = shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&sid) {
                    sess.accumulator.merge_update(id, input);
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    s.sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default()
                };
                drained_tool_calls.extend(pending_before);
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::ToolResult { content },
                });
            }
            AcpUpdateItem::TurnEnd => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    s.sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default()
                };
                drained_tool_calls.extend(pending_before);
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::TurnEnd,
                });
            }
        }
    }

    for (_id, name, input) in drained_tool_calls {
        let _ = event_tx.try_send(DriverEvent::Output {
            key: key.clone(),
            session_id: sid.clone(),
            run_id,
            item: AgentEventItem::ToolCall { name, input },
        });
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
            display_name: "test-opencode".to_string(),
            description: None,
            system_prompt: None,
            model: "openai/gpt-4o".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_probe_not_installed() {
        let driver = OpencodeDriver;
        let probe = driver.probe().await.unwrap();
        if probe.auth == ProbeAuth::NotInstalled {
            assert_eq!(probe.transport, TransportKind::AcpNative);
            assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_list_models_not_installed() {
        let driver = OpencodeDriver;
        if !command_exists("opencode") {
            let models = driver.list_models().await.unwrap();
            assert!(models.is_empty());
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_open_session_returns_idle() {
        let driver = OpencodeDriver;
        // Unique key: the driver's shared registry is process-global, so
        // re-running this test with the same key would re-bind to a stale
        // `OpencodeAgentProcess` from a previous case.
        let key = format!("opencode-test-open-session-{}", uuid::Uuid::new_v4());
        let result = driver
            .open_session(key, test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.session.process_state(), ProcessState::Idle));
    }

    #[test]
    fn build_mcp_chat_config_http_shape() {
        // Remote HTTP MCP shape — the only shape we produce.
        let config = build_mcp_chat_config("http://127.0.0.1:4321", "tok-xyz");
        assert_eq!(config["type"], "remote");
        assert_eq!(config["url"], "http://127.0.0.1:4321/mcp");
        assert!(config.get("command").is_none());
    }

    #[test]
    fn build_mcp_chat_config_trims_trailing_slash() {
        // Endpoint with trailing slash must not produce `//token/` in the URL.
        let config = build_mcp_chat_config("http://127.0.0.1:4321/", "tok-xyz");
        assert_eq!(config["url"], "http://127.0.0.1:4321/mcp");
    }

    // -----------------------------------------------------------------------
    // Multi-session unit tests (Phase 0.9 Stage 2)
    //
    // These exercise the in-process plumbing without a real `opencode` binary.
    // We construct a shared `OpencodeAgentProcess` by hand, wire up the stdin
    // channel to a test collector, and drive reader dispatch directly by
    // calling `classify_line` + `dispatch_line`. This mirrors the real reader
    // loop faithfully; the only difference is that no child is spawned.
    // -----------------------------------------------------------------------

    /// Build an `OpencodeAgentProcess` prepped for unit-test dispatch.
    /// Returns (process, stdin_rx, event_rx). The process is marked `started`
    /// so `new_session` / `resume_session` don't error out.
    fn build_test_process(
        key: &str,
    ) -> (
        Arc<OpencodeAgentProcess>,
        mpsc::Receiver<String>,
        tokio::sync::mpsc::Receiver<DriverEvent>,
    ) {
        let (events, event_tx) = EventFanOut::new();
        let event_rx = events.subscribe();
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);
        let proc = Arc::new(OpencodeAgentProcess {
            key: key.to_string(),
            events,
            event_tx,
            child: Mutex::new(None),
            stdin_tx: Mutex::new(Some(stdin_tx)),
            shared: Arc::new(Mutex::new(SharedReaderState::new())),
            // Matches production `ensure_process`: counter starts at 3.
            // Tests that simulate a bootstrap reservation burn id 3 via
            // `alloc_id()` before exercising secondary new_sessions.
            next_request_id: AtomicU64::new(3),
            reader_handles: Mutex::new(Vec::new()),
            started: std::sync::atomic::AtomicBool::new(true),
        });
        (proc, stdin_rx, event_rx)
    }

    /// Ping a line through the same code path the reader task uses.
    async fn feed_line(proc: &Arc<OpencodeAgentProcess>, line: &str) {
        let frame = classify_line(line, &proc.shared);
        let stdin_tx = {
            let guard = proc.stdin_tx.lock().unwrap();
            guard.clone().expect("stdin present")
        };
        dispatch_line(frame, &proc.key, &proc.event_tx, &proc.shared, &stdin_tx).await;
    }

    #[tokio::test]
    async fn new_session_mints_distinct_ids_via_live_child() {
        // Simulate: the bootstrap attach already minted session-1 via id 2.
        // Now call new_session twice — each should send a session/new on the
        // shared stdin and resolve with a fresh id carried on the response.
        let (proc, mut stdin_rx, _event_rx) = build_test_process("agent-1");

        // Drive two new_session calls in parallel: each awaits a oneshot
        // response the test will fulfill by feeding back a response line.
        let proc_a = proc.clone();
        let spec_a = test_spec();
        let new_a = tokio::spawn(async move { proc_a.request_new_session(&spec_a).await });
        let proc_b = proc.clone();
        let spec_b = test_spec();
        let new_b = tokio::spawn(async move { proc_b.request_new_session(&spec_b).await });

        // Collect the two outgoing session/new requests and extract their ids.
        let line_a = stdin_rx.recv().await.expect("first session/new on stdin");
        let line_b = stdin_rx.recv().await.expect("second session/new on stdin");
        let id_a = serde_json::from_str::<serde_json::Value>(&line_a).unwrap()["id"]
            .as_u64()
            .unwrap();
        let id_b = serde_json::from_str::<serde_json::Value>(&line_b).unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_ne!(id_a, id_b, "two session/new calls must use distinct ids");
        assert!(id_a >= 3 && id_b >= 3, "post-handshake ids must be >= 3");

        // Feed responses back through the reader path.
        let resp_a =
            format!(r#"{{"jsonrpc":"2.0","id":{id_a},"result":{{"sessionId":"sess-A"}}}}"#);
        let resp_b =
            format!(r#"{{"jsonrpc":"2.0","id":{id_b},"result":{{"sessionId":"sess-B"}}}}"#);
        feed_line(&proc, &resp_a).await;
        feed_line(&proc, &resp_b).await;

        let id_out_a = new_a.await.unwrap().unwrap();
        let id_out_b = new_b.await.unwrap().unwrap();
        assert_eq!(id_out_a, "sess-A");
        assert_eq!(id_out_b, "sess-B");
        assert_ne!(
            id_out_a, id_out_b,
            "new_session calls yield distinct session ids"
        );
    }

    #[tokio::test]
    async fn resume_session_preserves_supplied_id() {
        let (proc, mut stdin_rx, _event_rx) = build_test_process("agent-1");

        let proc_1 = proc.clone();
        let spec = test_spec();
        let resume =
            tokio::spawn(async move { proc_1.request_load_session(&spec, "stored-xyz").await });

        let line = stdin_rx.recv().await.expect("session/load on stdin");
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        let id = parsed["id"].as_u64().unwrap();
        assert_eq!(parsed["method"], "session/load");
        assert_eq!(parsed["params"]["sessionId"], "stored-xyz");

        // Respond with an empty result (some opencode versions do this),
        // forcing the fallback to the requested id.
        let resp = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#);
        feed_line(&proc, &resp).await;

        let id_out = resume.await.unwrap().unwrap();
        assert_eq!(id_out, "stored-xyz", "load fallback preserves supplied id");
    }

    #[tokio::test]
    async fn bootstrap_load_session_response_sends_deferred_prompt() {
        let (proc, mut stdin_rx, mut event_rx) = build_test_process("agent-bootstrap-load");
        let prompt_id = proc.alloc_id();
        assert_eq!(
            prompt_id, 3,
            "bootstrap deferred prompt should reserve id 3"
        );

        {
            let mut s = proc.shared.lock().unwrap();
            s.pending_requests.insert(
                2,
                PendingKind::LoadSession {
                    requested_session_id: "stored-xyz".to_string(),
                    responder: None,
                },
            );
            s.bootstrap_requested_session_id = Some("stored-xyz".to_string());
            s.bootstrap_pending_prompt = Some((prompt_id, "check your messages".to_string()));
        }

        let resp = r#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
        feed_line(&proc, resp).await;

        let prompt_line = tokio::time::timeout(Duration::from_secs(1), stdin_rx.recv())
            .await
            .expect("deferred prompt should be sent after bootstrap session/load")
            .expect("stdin channel should remain open");
        let prompt: serde_json::Value = serde_json::from_str(&prompt_line).unwrap();

        assert_eq!(prompt["id"], prompt_id);
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], "stored-xyz");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "check your messages");

        let mut saw_attached = false;
        let mut saw_prompt_in_flight = false;
        for _ in 0..4 {
            match tokio::time::timeout(Duration::from_millis(200), event_rx.recv()).await {
                Ok(Some(DriverEvent::SessionAttached { session_id, .. })) => {
                    assert_eq!(session_id, "stored-xyz");
                    saw_attached = true;
                }
                Ok(Some(DriverEvent::Lifecycle {
                    state: ProcessState::PromptInFlight { session_id, .. },
                    ..
                })) => {
                    assert_eq!(session_id, "stored-xyz");
                    saw_prompt_in_flight = true;
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }

        assert!(saw_attached, "bootstrap load should emit SessionAttached");
        assert!(
            saw_prompt_in_flight,
            "bootstrap load should transition to PromptInFlight after sending deferred prompt"
        );

        let s = proc.shared.lock().unwrap();
        assert!(
            s.bootstrap_pending_prompt.is_none(),
            "deferred prompt should be consumed"
        );
        assert!(
            matches!(
                s.sessions.get("stored-xyz").map(|slot| &slot.agent_state),
                Some(ProcessState::PromptInFlight { .. })
            ),
            "resumed bootstrap session should be marked PromptInFlight"
        );
    }

    #[tokio::test]
    async fn child_process_is_reused_across_sessions() {
        // Two `open_session` calls on the same key must hand back the same
        // `Arc<OpencodeAgentProcess>`.
        let driver = OpencodeDriver;
        let key = format!("opencode-test-reuse-{}", uuid::Uuid::new_v4());

        let s1 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        // Find the underlying process from the global registry.
        let proc1 = agent_instances().get(&key).expect("registered");

        // Mark started so the second open_session's request_new_session can
        // proceed. We can't actually spawn opencode in tests, but the invariant
        // we care about here is registry identity.
        proc1.started.store(true, Ordering::SeqCst);

        // Pre-wire a stdin_tx so request_new_session can write and we can
        // observe the outgoing request.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        *proc1.stdin_tx.lock().unwrap() = Some(stdin_tx);

        // Drive the second open_session (Secondary path) via the driver API.
        let driver_for_task = OpencodeDriver;
        let key_for_task = key.clone();
        let new_task: tokio::task::JoinHandle<anyhow::Result<SessionAttachment>> =
            tokio::spawn(async move {
                driver_for_task
                    .open_session(key_for_task, test_spec(), SessionIntent::New)
                    .await
            });

        // Fulfil the session/new response.
        let line = stdin_rx.recv().await.unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"]
            .as_u64()
            .unwrap();
        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"sess-reuse"}}}}"#);
        feed_line(&proc1, &resp).await;
        let s2 = new_task.await.unwrap().unwrap();

        // Second lookup: same process.
        let proc2 = agent_instances().get(&key).expect("registered");
        assert!(
            Arc::ptr_eq(&proc1, &proc2),
            "same agent key must map to the same OpencodeAgentProcess"
        );

        // Event stream identity: both open_session results share the same
        // fan-out — and therefore the same underlying child.
        assert!(
            Arc::ptr_eq(&s1.events.inner, &proc1.events.inner),
            "s1.events must share fan-out with the shared process"
        );
        assert!(
            Arc::ptr_eq(&s2.events.inner, &proc1.events.inner),
            "s2.events must share fan-out with the shared process"
        );
    }

    #[tokio::test]
    async fn session_update_events_carry_session_id() {
        // This test exercises the single most important multi-session
        // correctness invariant: a `session/update` frame landing on the
        // shared stdin must be routed to the session whose id is named by
        // `params.update.sessionId` — NOT to whichever session happens to
        // have a prompt in flight. We feed real JSON lines through the
        // same `classify_line` → `dispatch_line` path the production
        // reader uses, so any drift in `acp_protocol::parse_line`'s
        // SessionInit-at-items[0] contract would break this test.
        let (proc, _stdin_rx, mut event_rx) = build_test_process("agent-multi");

        // Seed two concurrent sessions as if `new_session` had minted them,
        // and put BOTH in PromptInFlight — this is the race the
        // `session_id`-based routing must disambiguate. If routing silently
        // fell back to "any session in PromptInFlight", the assertions
        // below would fail non-deterministically.
        let run_a = RunId::new_v4();
        let run_b = RunId::new_v4();
        {
            let mut s = proc.shared.lock().unwrap();
            let mut st_a = SessionRuntimeState::active("sess-A");
            st_a.run_id = Some(run_a);
            st_a.agent_state = ProcessState::PromptInFlight {
                run_id: run_a,
                session_id: "sess-A".to_string(),
            };
            s.sessions.insert("sess-A".to_string(), st_a);
            let mut st_b = SessionRuntimeState::active("sess-B");
            st_b.run_id = Some(run_b);
            st_b.agent_state = ProcessState::PromptInFlight {
                run_id: run_b,
                session_id: "sess-B".to_string(),
            };
            s.sessions.insert("sess-B".to_string(), st_b);
        }

        // Real `session/update` JSON for sess-A, as `opencode acp` would
        // emit it. `acp_protocol::parse_line` (post-00fc6d5) prepends
        // `AcpUpdateItem::SessionInit { session_id: "sess-A" }` at
        // items[0] — our routing reads that to pick the target session.
        let update_a = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-A","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi from A"}}}}"#;
        // And sess-B, same shape.
        let update_b = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-B","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi from B"}}}}"#;

        // Feed sess-A first, then sess-B, through the same code path the
        // reader uses. `feed_line` wraps `classify_line` + `dispatch_line`.
        feed_line(&proc, update_a).await;
        feed_line(&proc, update_b).await;

        // Drain events until we've seen one Text per session. With two
        // concurrent in-flight sessions, any misrouting (fallback to a
        // single in-flight session) would surface both texts on the same
        // `session_id` — exactly what this test forbids.
        let mut seen_a = false;
        let mut seen_b = false;
        for _ in 0..4 {
            let ev = match tokio::time::timeout(Duration::from_secs(1), event_rx.recv()).await {
                Ok(Some(ev)) => ev,
                _ => break,
            };
            if let DriverEvent::Output {
                session_id,
                run_id,
                item: AgentEventItem::Text { text },
                ..
            } = ev
            {
                match session_id.as_str() {
                    "sess-A" => {
                        assert_eq!(text, "hi from A", "sess-A text mismatch");
                        assert_eq!(run_id, run_a, "sess-A event must carry sess-A run id");
                        seen_a = true;
                    }
                    "sess-B" => {
                        assert_eq!(text, "hi from B", "sess-B text mismatch");
                        assert_eq!(run_id, run_b, "sess-B event must carry sess-B run id");
                        seen_b = true;
                    }
                    other => panic!("unexpected session_id on output event: {other}"),
                }
            }
            if seen_a && seen_b {
                break;
            }
        }
        assert!(
            seen_a && seen_b,
            "multi-session routing lost an event: seen_a={seen_a}, seen_b={seen_b}"
        );
    }

    #[tokio::test]
    async fn process_state_prefers_shared_session_state_over_stale_local_state() {
        let (proc, _stdin_rx, _event_rx) = build_test_process("agent-shared-state");
        let run_id = RunId::new_v4();
        let session_id = "sess-shared".to_string();

        {
            let mut shared = proc.shared.lock().unwrap();
            shared
                .sessions
                .insert(session_id.clone(), SessionRuntimeState::active(&session_id));
        }

        let handle = OpencodeHandle {
            key: proc.key.clone(),
            local_state: ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
            spec: test_spec(),
            proc,
            preassigned_session_id: Some(session_id.clone()),
            factory_path: FactoryPath::Secondary,
        };

        assert!(
            matches!(
                handle.process_state(),
                ProcessState::Active { session_id: active_session_id }
                    if active_session_id == session_id
            ),
            "shared session state should be authoritative when local state is stale"
        );
    }

    #[tokio::test]
    async fn bootstrap_deferred_prompt_id_does_not_collide_with_racing_new_session() {
        // Regression for the id-3 collision: before the fix, the bootstrap
        // path hardcoded `id = 3` for its deferred init prompt while
        // `next_request_id` also started at 3 — a secondary `new_session`
        // racing the bootstrap's `session/new` response would receive the
        // same id via `alloc_id()`. Now id 3 is reserved up-front.
        //
        // We simulate the bootstrap reservation by burning id 3 off the
        // counter (as `start_bootstrap_child` does when an init_prompt is
        // present), then call `request_new_session` twice and assert the
        // outgoing ids are distinct from the reserved 3 and from each other.
        let (proc, mut stdin_rx, _event_rx) = build_test_process("agent-race");

        // Mimic bootstrap: the reserved deferred-prompt id is 3.
        let reserved = proc.alloc_id();
        assert_eq!(reserved, 3, "bootstrap reservation should be id 3");

        // Two racing secondary new_sessions.
        let proc_a = proc.clone();
        let proc_b = proc.clone();
        let spec_a = test_spec();
        let spec_b = test_spec();
        let a = tokio::spawn(async move { proc_a.request_new_session(&spec_a).await });
        let b = tokio::spawn(async move { proc_b.request_new_session(&spec_b).await });

        let line_a = stdin_rx.recv().await.expect("first session/new");
        let line_b = stdin_rx.recv().await.expect("second session/new");
        let id_a = serde_json::from_str::<serde_json::Value>(&line_a).unwrap()["id"]
            .as_u64()
            .unwrap();
        let id_b = serde_json::from_str::<serde_json::Value>(&line_b).unwrap()["id"]
            .as_u64()
            .unwrap();

        assert_ne!(
            id_a, reserved,
            "secondary new_session id must not collide with reserved deferred-prompt id"
        );
        assert_ne!(
            id_b, reserved,
            "secondary new_session id must not collide with reserved deferred-prompt id"
        );
        assert_ne!(
            id_a, id_b,
            "two concurrent new_session calls must use distinct ids"
        );

        // Drain the futures cleanly.
        let resp_a =
            format!(r#"{{"jsonrpc":"2.0","id":{id_a},"result":{{"sessionId":"sess-A"}}}}"#);
        let resp_b =
            format!(r#"{{"jsonrpc":"2.0","id":{id_b},"result":{{"sessionId":"sess-B"}}}}"#);
        feed_line(&proc, &resp_a).await;
        feed_line(&proc, &resp_b).await;
        let _ = a.await.unwrap().unwrap();
        let _ = b.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn open_session_new_before_child_started_returns_bootstrap_handle() {
        // `open_session(New)` when no child is running returns a Bootstrap
        // Idle handle — callers must call `run()` to bring the child online.
        // (The old `new_session` shim would error here; `open_session` is
        // intentionally lenient so the caller decides the lifecycle.)
        let driver = OpencodeDriver;
        let key = format!("opencode-test-no-child-{}", uuid::Uuid::new_v4());
        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(
            matches!(result.session.process_state(), ProcessState::Idle),
            "expected Idle handle before child is started"
        );
        agent_instances().remove(&key);
    }

    /// Regression: `DriverEvent::SessionAttached` must be emitted EXACTLY ONCE
    /// per secondary `new_session`. Before the fix, the reader task in
    /// `dispatch_line` emitted SessionAttached unconditionally on a
    /// `NewSessionResponse`, AND the secondary `OpencodeHandle::start()` path
    /// also emitted it — producing two events per secondary session. Bootstrap
    /// minted a single event (its start path is different), so the asymmetry
    /// broke `await_session_attached` consumers: draining one event left a
    /// duplicate in the buffer that later consumers mistook for a different
    /// session's attach. Mirrors kimi's split: reader emits for
    /// warmup/bootstrap only; handle.run() emits for secondary.
    #[tokio::test]
    async fn session_attached_emitted_exactly_once_per_secondary_new_session() {
        let (proc, mut stdin_rx, mut event_rx) = build_test_process("agent-attach-once");

        // Full secondary path: `request_new_session` drives the session/new
        // RPC (whose response arrives via the reader path), then construct a
        // secondary `OpencodeHandle` and call its `run()` — the same shape
        // the driver's `open_session` entrypoint produces.
        let proc_c = proc.clone();
        let spec = test_spec();
        let call = tokio::spawn(async move { proc_c.request_new_session(&spec).await });

        let line = stdin_rx
            .recv()
            .await
            .expect("secondary session/new on stdin");
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_u64().unwrap();
        assert_eq!(req["method"], "session/new");

        // Response with `responder = Some(tx)` at this pending id → routes to
        // the NewSessionResponse secondary arm. Under the fix, the reader
        // must NOT emit SessionAttached here.
        let sid = "sess-once";
        let resp = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"{sid}"}}}}"#);
        feed_line(&proc, &resp).await;

        let got = call.await.unwrap().unwrap();
        assert_eq!(got, sid);

        // Build the secondary handle exactly as `OpencodeDriver::new_session`
        // would, and run its `start()` — this is the OTHER emit site. With
        // the fix, `start()` emits exactly once and the reader emits zero,
        // so the fan-out sees a single SessionAttached for `sid`.
        let mut handle = OpencodeHandle {
            key: proc.key.clone(),
            local_state: ProcessState::Idle,
            spec: test_spec(),
            proc: Arc::clone(&proc),
            preassigned_session_id: Some(sid.to_string()),
            factory_path: FactoryPath::Secondary,
        };
        handle.run(None).await.expect("secondary run()");

        // Drain the event receiver and count SessionAttached events for
        // `sid`. Before the fix this would be 2; with the fix it's 1.
        let mut attached = 0;
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(150), event_rx.recv()).await
            {
                Ok(Some(DriverEvent::SessionAttached { session_id, .. })) if session_id == sid => {
                    attached += 1;
                }
                Ok(Some(_)) => {} // ignore other events
                _ => break,       // timeout or channel closed
            }
        }
        assert_eq!(
            attached, 1,
            "secondary open_session must emit SessionAttached exactly once \
             (reader emits for bootstrap only; handle.run() emits for \
             secondary); saw {attached}"
        );
    }

    /// Regression for the Stage 2 ship-blocker: closing the bootstrap handle
    /// while a secondary session is still live (mid-prompt) must NOT tear
    /// down the shared opencode child, its reader tasks, or the fan-out.
    /// Before the fix, the bootstrap close path called `kill_child` +
    /// `events.close()` + `agent_instances().remove()` unconditionally —
    /// which SIGTERM'd the shared child, closed the fan-out, and pruned the
    /// registry while a live secondary was still emitting events into both.
    /// The fix gates teardown on "all sessions closed" regardless of role.
    #[tokio::test]
    async fn bootstrap_close_with_live_secondary_does_not_tear_down_shared_child() {
        let driver = OpencodeDriver;
        let key = format!("opencode-bootstrap-live-secondary-{}", uuid::Uuid::new_v4());

        // Bring up a shared process via the driver (registers it + builds
        // the fan-out). Also mark started so secondary construction works.
        let s0 = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        let proc = agent_instances().get(&key).expect("registered");
        proc.started.store(true, Ordering::SeqCst);

        // Seed a "live" shared child: a parked reader + a non-None child
        // slot are the two things the teardown path would mutate. We can't
        // spawn a real opencode, so we use a sleeping `sh` and a parked
        // JoinHandle as stand-ins and verify the teardown either touches
        // them (secondary close) or doesn't (bootstrap close with live
        // secondary).
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);
        *proc.stdin_tx.lock().unwrap() = Some(stdin_tx);
        let parked_reader = tokio::spawn(async {
            let () = std::future::pending().await;
        });
        proc.reader_handles.lock().unwrap().push(parked_reader);

        // Seed two sessions in shared state:
        //   - bootstrap_sid: recorded in `bootstrap_session_id` so bootstrap
        //     close can identify its own slot.
        //   - secondary_sid: PromptInFlight, modelling the race where the
        //     user hits "close" on one tab while another tab's prompt is
        //     still streaming.
        let bootstrap_sid = "sess-bootstrap".to_string();
        let secondary_sid = "sess-secondary".to_string();
        let secondary_run = RunId::new_v4();
        {
            let mut s = proc.shared.lock().unwrap();
            s.sessions.insert(
                bootstrap_sid.clone(),
                SessionRuntimeState::active(&bootstrap_sid),
            );
            let mut sec = SessionRuntimeState::active(&secondary_sid);
            sec.run_id = Some(secondary_run);
            sec.agent_state = ProcessState::PromptInFlight {
                run_id: secondary_run,
                session_id: secondary_sid.clone(),
            };
            s.sessions.insert(secondary_sid.clone(), sec);
            s.bootstrap_session_id = Some(bootstrap_sid.clone());
        }

        let events_handle = s0.events.clone();
        let mut bootstrap_handle = s0.session;

        // Build a secondary handle by hand (bypassing request_new_session,
        // which would need a live reader to respond). Mirror what
        // `new_session` produces: preassigned session id + Secondary role,
        // plus the Active local_state the reader's SessionAttached handler
        // would flip us into.
        let mut secondary_handle = OpencodeHandle {
            key: key.clone(),
            local_state: ProcessState::PromptInFlight {
                run_id: secondary_run,
                session_id: secondary_sid.clone(),
            },
            spec: test_spec(),
            proc: Arc::clone(&proc),
            preassigned_session_id: Some(secondary_sid.clone()),
            factory_path: FactoryPath::Secondary,
        };

        // ---- Close the bootstrap while the secondary is mid-prompt. ----
        bootstrap_handle.close().await.unwrap();

        // Shared child bits must remain intact for the secondary. We
        // couldn't install a real child (can't spawn opencode in tests),
        // so the teardown signals we check are the ones `kill_child` +
        // the post-kill cleanup mutate: reader_handles (aborted + drained),
        // `events.closing` (fan-out drain flag), and the registry entry.
        assert_eq!(
            proc.reader_handles.lock().unwrap().len(),
            1,
            "bootstrap close with a live secondary must NOT abort the shared reader handles"
        );
        assert!(
            !proc.reader_handles.lock().unwrap()[0].is_finished(),
            "parked reader must still be running"
        );
        assert!(
            !events_handle.inner.closing.load(Ordering::SeqCst),
            "bootstrap close with a live secondary must NOT close the fan-out"
        );
        assert!(
            agent_instances().get(&key).is_some(),
            "bootstrap close with a live secondary must NOT prune the registry entry"
        );
        // The bootstrap slot should be gone; secondary slot still live.
        {
            let s = proc.shared.lock().unwrap();
            assert!(
                !s.sessions.contains_key(&bootstrap_sid),
                "bootstrap close must drop its own session slot"
            );
            assert!(
                matches!(
                    s.sessions.get(&secondary_sid).map(|slot| &slot.agent_state),
                    Some(ProcessState::PromptInFlight { .. })
                ),
                "secondary slot must remain mid-prompt after bootstrap close"
            );
        }

        // ---- Close the secondary. Now teardown fires. ----
        secondary_handle.close().await.unwrap();

        assert!(
            proc.reader_handles.lock().unwrap().is_empty(),
            "last-session close must drain shared reader handles"
        );
        assert!(
            events_handle.inner.closing.load(Ordering::SeqCst),
            "last-session close must signal the fan-out to drain"
        );
        assert!(
            agent_instances().get(&key).is_none(),
            "last-session close must prune the registry entry"
        );

        // Best-effort cleanup in case anything lingered.
        agent_instances().remove(&key);
    }

    /// Regression: bootstrap `session/new` response that omits `sessionId`
    /// is a protocol violation, not something to paper over with a
    /// synthesized UUID. The runtime never created a session for that fake
    /// id, so any downstream prompt/resume targeting it would fail
    /// silently. Verify we emit `Failed { AgentError::Protocol }`, do NOT
    /// emit `SessionAttached`, and do NOT seed shared state with a bogus
    /// entry.
    #[tokio::test]
    async fn new_session_response_missing_session_id_surfaces_protocol_error() {
        let (proc, _stdin_rx, mut event_rx) = build_test_process("agent-proto");

        // Pre-register a bootstrap-path pending entry for id 2 (responder=None).
        {
            let mut s = proc.shared.lock().unwrap();
            s.pending_requests
                .insert(2, PendingKind::NewSession { responder: None });
        }

        // Feed a `session/new` response missing `sessionId`.
        let resp = r#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
        feed_line(&proc, resp).await;

        // Drain events: we expect Failed(Protocol) and no SessionAttached.
        let mut saw_failed = false;
        for _ in 0..4 {
            match tokio::time::timeout(Duration::from_millis(200), event_rx.recv()).await {
                Ok(Some(DriverEvent::Failed { error, run_id, .. })) => {
                    assert!(
                        matches!(error, AgentError::Protocol(_)),
                        "expected AgentError::Protocol, got {error:?}"
                    );
                    assert_eq!(
                        run_id,
                        uuid::Uuid::nil(),
                        "bootstrap failure carries nil RunId"
                    );
                    saw_failed = true;
                }
                Ok(Some(DriverEvent::SessionAttached { .. })) => {
                    panic!(
                        "must NOT emit SessionAttached for a spec-violating session/new response"
                    );
                }
                Ok(Some(DriverEvent::Lifecycle {
                    state: ProcessState::Active { .. },
                    ..
                })) => {
                    panic!("must NOT transition to Active on a missing sessionId");
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(
            saw_failed,
            "bootstrap path must surface AgentError::Protocol via Failed"
        );

        // Shared state must not have seeded a bogus session entry.
        let s = proc.shared.lock().unwrap();
        assert!(
            s.sessions.is_empty(),
            "no SessionRuntimeState should be seeded when sessionId is missing"
        );
        assert!(
            s.bootstrap_session_id.is_none(),
            "bootstrap_session_id must remain unset"
        );
    }

    /// Regression: secondary `new_session` path (responder = Some(tx))
    /// with a spec-violating response must resolve the oneshot with an
    /// Err whose message names the protocol violation — the caller's
    /// `new_session()` must not return a bogus id.
    #[tokio::test]
    async fn secondary_new_session_missing_session_id_returns_err() {
        let (proc, _stdin_rx, _event_rx) = build_test_process("agent-proto-sec");

        // Simulate a secondary new_session in flight: responder present.
        let (responder, rx) = oneshot::channel::<anyhow::Result<String>>();
        {
            let mut s = proc.shared.lock().unwrap();
            s.pending_requests.insert(
                7,
                PendingKind::NewSession {
                    responder: Some(responder),
                },
            );
        }

        // Spec-violating response.
        let resp = r#"{"jsonrpc":"2.0","id":7,"result":{}}"#;
        feed_line(&proc, resp).await;

        let outcome = tokio::time::timeout(Duration::from_millis(500), rx)
            .await
            .expect("responder resolved")
            .expect("sender not dropped");
        let err = outcome.expect_err("missing sessionId must not resolve Ok");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("protocol violation"),
            "error message should mention protocol violation; got: {msg}"
        );

        // Shared state must remain clean — no bogus entries, no bootstrap
        // session id stashed.
        let s = proc.shared.lock().unwrap();
        assert!(
            s.sessions.is_empty(),
            "secondary path must not seed a session entry"
        );
        assert!(
            s.bootstrap_session_id.is_none(),
            "secondary path never touches bootstrap_session_id"
        );
    }

    /// Regression: `start_bootstrap_child` installs pending-request entries
    /// BEFORE writing the handshake. If a write fails (child exited
    /// between spawn and write), we must roll back those entries — leaving
    /// them behind poisons the cached `Arc<OpencodeAgentProcess>` that a
    /// subsequent `attach()` reuses (because `is_stale()` returns `false`
    /// before `started` is flipped).
    ///
    /// The rollback path in `start_bootstrap_child` is synchronous shared-
    /// state cleanup keyed off the installed ids. We can't trigger the
    /// write failure without spawning a real `opencode` process (the
    /// command is hardcoded), so we reproduce the state transitions the
    /// rollback promises: install + rollback and verify shared state
    /// returns to empty. This matches the actual rollback block byte-for-
    /// byte; if someone rewrites it and forgets an entry, this test fails.
    #[tokio::test]
    async fn start_bootstrap_handshake_write_failure_clears_pending_state() {
        let (proc, _stdin_rx, _event_rx) = build_test_process("agent-rollback");

        // --- Simulate the pre-write install block in start_bootstrap_child. ---
        let deferred_prompt_id: u64 = proc.alloc_id();
        {
            let mut s = proc.shared.lock().unwrap();
            s.pending_requests.insert(1, PendingKind::Initialize);
            s.pending_requests
                .insert(2, PendingKind::NewSession { responder: None });
            s.bootstrap_requested_session_id = Some("would-resume".to_string());
            s.bootstrap_pending_prompt =
                Some((deferred_prompt_id, "deferred prompt text".to_string()));
        }

        // Sanity: state is installed.
        {
            let s = proc.shared.lock().unwrap();
            assert!(s.pending_requests.contains_key(&1));
            assert!(s.pending_requests.contains_key(&2));
            assert!(s.bootstrap_requested_session_id.is_some());
            assert!(s.bootstrap_pending_prompt.is_some());
        }

        // --- Simulate the rollback block (handshake write failed). ---
        {
            let mut s = proc.shared.lock().unwrap();
            s.pending_requests.remove(&1);
            s.pending_requests.remove(&2);
            s.bootstrap_requested_session_id = None;
            s.bootstrap_pending_prompt = None;
            s.pending_requests.remove(&deferred_prompt_id);
        }

        // --- Post-rollback shared state must be empty of handshake scaffolding. ---
        let s = proc.shared.lock().unwrap();
        assert!(
            s.pending_requests.is_empty(),
            "rollback must purge all handshake pending entries; got: {:?}",
            s.pending_requests.keys().collect::<Vec<_>>()
        );
        assert!(
            s.bootstrap_requested_session_id.is_none(),
            "bootstrap_requested_session_id must be cleared on rollback"
        );
        assert!(
            s.bootstrap_pending_prompt.is_none(),
            "bootstrap_pending_prompt must be cleared on rollback"
        );
        assert!(
            s.sessions.is_empty(),
            "no session slot was seeded before the write — must stay empty"
        );
    }

    // -----------------------------------------------------------------------
    // Task 6 behavioral tests: open_session + run
    // -----------------------------------------------------------------------

    /// open_session(New) on no-child-yet → Bootstrap path.
    /// Contract:
    ///   - session_id() == None (no wire I/O, nothing minted)
    ///   - mock stdin saw zero writes
    ///   - no DriverEvent emitted
    #[tokio::test]
    async fn open_session_new_on_no_child_bootstrap_path() {
        let driver = OpencodeDriver;
        let key = format!("oc-open-no-child-{}", uuid::Uuid::new_v4());

        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();

        // session_id() must be None — no wire I/O happened.
        assert!(
            result.session.session_id().is_none(),
            "bootstrap open_session(New) must return handle with session_id == None"
        );

        // No events should have been emitted by the factory.
        let mut event_rx = result.events.subscribe();
        match tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await {
            Err(_timeout) => {} // expected: no events
            Ok(Some(ev)) => panic!(
                "open_session must not emit any DriverEvent; got: {:?}",
                std::mem::discriminant(&ev)
            ),
            Ok(None) => {} // channel closed before any event — also fine
        }

        // Clean up registry entry.
        agent_instances().remove(&key);
    }

    /// open_session(New) on live child → Secondary path.
    /// Contract:
    ///   - session_id().is_some() (factory minted it via session/new wire RPC)
    ///   - mock stdin saw exactly one session/new request
    ///   - no DriverEvent emitted by the factory call itself
    #[tokio::test]
    async fn open_session_new_on_live_child_secondary_path() {
        let (proc, mut stdin_rx, mut event_rx) = build_test_process("oc-open-live");

        let driver = OpencodeDriver;
        let key = proc.key.clone();

        // Register the process in the global registry so `ensure_process` finds
        // it (required for the driver's open_session to reach it).
        agent_instances().get_or_init(&key, || Arc::clone(&proc));

        // Launch open_session in a task — it will block waiting for the
        // session/new response.
        let key_c = key.clone();
        let open_task = tokio::spawn(async move {
            driver
                .open_session(key_c, test_spec(), SessionIntent::New)
                .await
        });

        // Factory must have written exactly one session/new to stdin.
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), stdin_rx.recv())
            .await
            .expect("open_session must write session/new to stdin within 2s")
            .expect("stdin channel open");

        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            req["method"], "session/new",
            "first stdin write must be session/new"
        );

        // No more stdin writes should arrive before we respond.
        match tokio::time::timeout(std::time::Duration::from_millis(50), stdin_rx.recv()).await {
            Err(_) => {} // expected: only one write
            Ok(msg) => panic!("unexpected stdin write from open_session factory: {msg:?}"),
        }

        // Feed back the session/new response to unblock open_session.
        let id = req["id"].as_u64().unwrap();
        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"live-sess-1"}}}}"#);
        feed_line(&proc, &resp).await;

        let result = open_task.await.unwrap().unwrap();

        // session_id() must be Some — factory minted it.
        assert_eq!(
            result.session.session_id(),
            Some("live-sess-1"),
            "Secondary open_session(New) must return handle with minted session_id"
        );

        // No DriverEvent must have been emitted by the factory.
        // (The reader does emit events when it processes the session/new
        // response for the bootstrap path, but for Secondary with a responder
        // it must NOT emit SessionAttached — that's run()'s job.)
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await
            {
                Err(_) => break, // no events: good
                Ok(Some(DriverEvent::SessionAttached { .. })) => {
                    panic!("open_session Secondary factory must NOT emit SessionAttached");
                }
                Ok(Some(DriverEvent::Lifecycle {
                    state: ProcessState::Active { .. },
                    ..
                })) => {
                    panic!("open_session Secondary factory must NOT emit Active lifecycle");
                }
                Ok(Some(_other)) => {} // ignore other events (e.g. unrelated Lifecycle)
                Ok(None) => break,
            }
        }

        agent_instances().remove(&key);
    }

    /// After run() on the live-child (Secondary) case:
    ///   - first emitted event is SessionAttached
    ///   - mock stdin saw NO second session/new (factory already did it)
    #[tokio::test]
    async fn run_secondary_emits_session_attached_no_second_session_new() {
        let (proc, mut stdin_rx, mut event_rx) = build_test_process("oc-run-secondary");

        let driver = OpencodeDriver;
        let key = proc.key.clone();
        agent_instances().get_or_init(&key, || Arc::clone(&proc));

        // Open a secondary session (blocks on session/new response).
        let key_c = key.clone();
        let open_task = tokio::spawn(async move {
            driver
                .open_session(key_c, test_spec(), SessionIntent::New)
                .await
        });

        // Drain the factory's session/new write and respond.
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), stdin_rx.recv())
            .await
            .expect("open_session must write session/new")
            .expect("stdin open");
        let id = serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"]
            .as_u64()
            .unwrap();
        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"sess-run-sec"}}}}"#);
        feed_line(&proc, &resp).await;

        let result = open_task.await.unwrap().unwrap();
        let mut handle = result.session;

        // Now call run() — this is the Secondary run path.
        handle.run(None).await.expect("run() must succeed");

        // First meaningful event after run() must be SessionAttached.
        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("run() must emit at least one event")
            .expect("channel open");

        // Skip any Starting lifecycle that run() emits first.
        let first_meaningful = if let DriverEvent::Lifecycle {
            state: ProcessState::Starting,
            ..
        } = &ev
        {
            tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
                .await
                .expect("run() must emit SessionAttached after Starting")
                .expect("channel open")
        } else {
            ev
        };

        assert!(
            matches!(
                first_meaningful,
                DriverEvent::SessionAttached { ref session_id, .. } if session_id == "sess-run-sec"
            ),
            "first non-Starting event after run() must be SessionAttached for 'sess-run-sec'; got: {:?}",
            std::mem::discriminant(&first_meaningful)
        );

        // No second session/new must have been written to stdin.
        match tokio::time::timeout(std::time::Duration::from_millis(100), stdin_rx.recv()).await {
            Err(_) => {} // expected: no extra writes
            Ok(Some(msg)) => {
                let val: serde_json::Value =
                    serde_json::from_str(&msg).unwrap_or(serde_json::Value::Null);
                if val.get("method") == Some(&serde_json::Value::String("session/new".to_string()))
                {
                    panic!("run() Secondary must NOT send a second session/new; stdin got: {msg}");
                }
                // Other writes (e.g. session/prompt from init_prompt) are fine.
            }
            Ok(None) => {} // channel closed: fine
        }

        agent_instances().remove(&key);
    }
}
