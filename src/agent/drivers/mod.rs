//! Runtime driver abstraction.
//!
//! This module contains the trait and type scaffolding for the
//! driver layer backing every agent runtime in production.

pub mod acp_protocol;
pub mod claude;
pub mod claude_headless;
pub mod codex;
pub mod codex_app_server;
pub mod fake;
pub mod gemini;
pub mod kimi;
pub mod opencode;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent::AgentRuntime;
use crate::store::agents::AgentEnvVar;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Stable identifier for an agent within a runtime host.
///
/// Typically the agent's persisted UUID (as a string). String-typed so we can
/// use it as a `HashMap` key and ship it through events without allocating a
/// newtype — the code already treats this as a `String`.
pub type AgentKey = String;

/// Runtime-assigned session identifier (Claude's `sessionId`, Codex's session
/// id, etc.). Runtimes own the canonical form; we never synthesize one.
pub type SessionId = String;

/// Identifier assigned per prompt-in-flight. Lets UI/backend correlate a
/// `prompt` call with its `Completed`/`Failed` event.
pub type RunId = uuid::Uuid;

// ---------------------------------------------------------------------------
// Capability bitflags
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Optional features a runtime may advertise during probing.
    ///
    /// Used by [`RuntimeProbe`] to tell the agent manager which surfaces
    /// (login, session list, resume, cancel, etc.) to expose in the UI.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CapabilitySet: u32 {
        const LOGIN          = 1 << 0;
        const SESSION_LIST   = 1 << 1;
        const RESUME_SESSION = 1 << 2;
        const CANCEL         = 1 << 3;
        const SLASH_COMMANDS = 1 << 4;
        const MODEL_LIST     = 1 << 5;
    }
}

// ---------------------------------------------------------------------------
// Enums — probe / auth / transport
// ---------------------------------------------------------------------------

/// Outcome of probing a runtime's installed authentication state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeAuth {
    /// CLI or adapter binary is missing on this host.
    NotInstalled,
    /// Binary is present but the user has not completed a login flow.
    Unauthed,
    /// Binary is present and credentials are valid.
    Authed,
}

/// Result of running a runtime-specific login flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// Login finished successfully.
    Completed,
    /// Login is waiting on the user (e.g. browser open, device code pending).
    PendingUserAction { message: String },
    /// Login failed with a runtime-reported reason.
    Failed { reason: String },
}

/// Transport strategy a driver uses to talk to its runtime.
///
/// - `AcpAdapter`: external adapter binary speaking ACP over stdio.
/// - `AcpNative`: runtime speaks ACP natively (e.g. Kimi CLI).
/// - `StreamJson`: bespoke streaming-JSON protocol (e.g. Claude raw).
/// - `HttpAppServer`: app server model (e.g. OpenCode daemon).
/// - `CodexAppServer`: native app-server protocol over stdio (JSONL, no `jsonrpc` header on wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    AcpAdapter,
    AcpNative,
    StreamJson,
    HttpAppServer,
    /// Codex app-server protocol — JSONL over stdio. Wire format omits the
    /// `"jsonrpc":"2.0"` header present in ACP messages.
    CodexAppServer,
}

// ---------------------------------------------------------------------------
// Agent state + runtime errors
// ---------------------------------------------------------------------------

/// Driver-facing error type surfaced on event streams and cancel outcomes.
///
/// Plain-data — carrying `String` payloads so the error crosses thread
/// boundaries cheaply and `Clone`s for fan-out to multiple subscribers.
/// `PartialEq` is deliberately NOT derived: downstream code should match on
/// the variant, not equality-compare error bodies.
#[derive(Debug, Clone)]
pub enum AgentError {
    /// Transport-layer failure (process exited, stdio closed, socket died).
    Transport(String),
    /// Protocol violation (malformed ACP frame, unexpected message, etc.).
    Protocol(String),
    /// Operation timed out waiting for a runtime response.
    Timeout,
    /// Runtime reported a domain error via its protocol.
    RuntimeReported(String),
}

/// Lifecycle state of an agent handle as observed by the manager.
#[derive(Debug, Clone)]
pub enum AgentState {
    /// Handle constructed but not yet started.
    Idle,
    /// Handle is spinning up the runtime process.
    Starting,
    /// Runtime is live with a session; no prompt currently in flight.
    Active { session_id: SessionId },
    /// A prompt is currently executing on the live session.
    PromptInFlight {
        run_id: RunId,
        session_id: SessionId,
    },
    /// Handle has been closed and cannot be reused.
    Closed,
    /// Handle is in a non-recoverable error state.
    Failed(AgentError),
}

// ---------------------------------------------------------------------------
// Events produced by drivers
// ---------------------------------------------------------------------------

/// Individual turn content item emitted during a prompt's execution.
///
/// Mirrors the v1 [`crate::agent::drivers::ParsedEvent`] shape but scoped to
/// what belongs inside a single run's output. Session-level concerns move up
/// to [`DriverEvent`].
#[derive(Debug, Clone)]
pub enum AgentEventItem {
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    /// Transport layers MUST coalesce ACP's deferred `tool_call_update` frames
    /// into this variant before emitting; callers never see partial tool-call
    /// input.
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        content: String,
    },
    TurnEnd,
}

/// Terminal condition of a completed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    /// Runtime finished the turn normally.
    Natural,
    /// Run was cancelled by a `cancel()` call before completion.
    Cancelled,
    /// Transport closed mid-run (process exit, stdio EOF, etc.).
    TransportClosed,
}

/// Result payload delivered alongside a [`DriverEvent::Completed`].
#[derive(Debug, Clone)]
pub struct RunResult {
    pub finish_reason: FinishReason,
}

/// Top-level event published on a handle's event stream.
///
/// Subscribers (AgentManager, telemetry, UI) consume these; the fan-out
/// machinery that dispatches them lands in Task 2.
#[derive(Debug, Clone)]
pub enum DriverEvent {
    /// Handle transitioned to a new lifecycle state.
    Lifecycle { key: AgentKey, state: AgentState },
    /// A session was attached (either new or resumed) for this key.
    SessionAttached {
        key: AgentKey,
        session_id: SessionId,
    },
    /// A single output item from an in-flight run. `session_id` is the run's
    /// parent session; runs are turns inside sessions.
    Output {
        key: AgentKey,
        session_id: SessionId,
        run_id: RunId,
        item: AgentEventItem,
    },
    /// A run completed successfully.
    Completed {
        key: AgentKey,
        session_id: SessionId,
        run_id: RunId,
        result: RunResult,
    },
    /// A run failed.
    Failed {
        key: AgentKey,
        session_id: SessionId,
        run_id: RunId,
        error: AgentError,
    },
}

// ---------------------------------------------------------------------------
// Event stream / fan-out
// ---------------------------------------------------------------------------

/// Inbound channel capacity: transport tasks write here, dispatcher reads.
///
/// Sized generously so bursts of output items (which arrive in tight loops
/// from a runtime's stdout reader) don't push back on the transport task.
const INBOUND_CAPACITY: usize = 512;

/// Per-observer channel capacity: dispatcher writes here, each subscriber
/// drains at its own pace. On overflow we drop + warn + bump a metric rather
/// than back-pressure the dispatcher (see back-pressure policy in the design
/// doc).
const OBSERVER_CAPACITY: usize = 256;

/// Fan-out dispatcher that distributes [`DriverEvent`]s from a single inbound
/// queue to every registered observer.
///
/// Lifecycle:
///  - Construct via [`EventFanOut::new`], which yields an [`EventStreamHandle`]
///    and the inbound `mpsc::Sender<DriverEvent>` used by transport tasks.
///  - The first call to [`EventStreamHandle::subscribe`] spawns the dispatcher
///    task (exactly once) and returns a bounded per-observer receiver.
///  - When the agent closes, [`EventStreamHandle::close`] flips the closing
///    flag; the dispatcher drains any remaining inbound events and exits.
///
/// Back-pressure: observers get a bounded 256-deep queue. A full queue results
/// in `try_send` returning `Full`, which drops the event, logs a warning, and
/// increments `chorus_driver_events_dropped`. This keeps a single slow
/// subscriber (e.g. UI client) from stalling the transport reader.
pub struct EventFanOut {
    /// Receiver for events emitted by handle transport tasks. Taken by the
    /// dispatcher task on first `subscribe()`; `Mutex<Option<_>>` so the
    /// "spawn exactly once" check is a single atomic take.
    inbound_rx: Mutex<Option<mpsc::Receiver<DriverEvent>>>,
    /// Observer senders. Guarded by `std::sync::RwLock` so that `subscribe()`
    /// adds entries without blocking the dispatcher's (read-lock) send loop.
    /// Never held across `.await`.
    observers: Arc<RwLock<Vec<mpsc::Sender<DriverEvent>>>>,
    /// Set by [`EventStreamHandle::close`]. Dispatcher terminates when this
    /// is true AND the inbound receiver is drained.
    closing: AtomicBool,
}

impl std::fmt::Debug for EventFanOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventFanOut")
            .field(
                "observers",
                &self.observers.read().map(|g| g.len()).unwrap_or(0),
            )
            .field("closing", &self.closing.load(Ordering::SeqCst))
            .finish()
    }
}

/// Return a static string naming the `DriverEvent` variant for logging.
///
/// Used in place of `std::mem::discriminant(&event)` so warn-level log lines
/// carry a human-readable kind (`"Output"`, `"Completed"`, …) rather than the
/// opaque `Discriminant(...)` debug print.
fn event_kind_name(e: &DriverEvent) -> &'static str {
    match e {
        DriverEvent::Lifecycle { .. } => "Lifecycle",
        DriverEvent::SessionAttached { .. } => "SessionAttached",
        DriverEvent::Output { .. } => "Output",
        DriverEvent::Completed { .. } => "Completed",
        DriverEvent::Failed { .. } => "Failed",
    }
}

impl EventFanOut {
    /// Construct a fan-out pair: the stream handle that observers subscribe
    /// to, plus the inbound sender that internal transport tasks write to.
    /// Inbound channel capacity is [`INBOUND_CAPACITY`] (512).
    ///
    /// `new` returns a pair rather than `Self` because the inbound sender is
    /// unavoidably the symmetric half of the fan-out: transport tasks need it
    /// before any observer exists, and the fan-out's internal receiver must
    /// be owned by the dispatcher we spawn later.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> (EventStreamHandle, mpsc::Sender<DriverEvent>) {
        let (tx, rx) = mpsc::channel(INBOUND_CAPACITY);
        let fanout = Arc::new(EventFanOut {
            inbound_rx: Mutex::new(Some(rx)),
            observers: Arc::new(RwLock::new(Vec::new())),
            closing: AtomicBool::new(false),
        });
        (EventStreamHandle { inner: fanout }, tx)
    }

    /// Spawn the dispatcher task exactly once per `EventFanOut`. Subsequent
    /// calls are a no-op: the `Option::take` on `inbound_rx` returns `None`
    /// for every call after the first.
    fn spawn_dispatcher_once(self: &Arc<Self>) {
        let rx_opt = {
            let mut guard = self.inbound_rx.lock().unwrap();
            guard.take()
        };
        let Some(mut rx) = rx_opt else {
            // Dispatcher already spawned.
            return;
        };
        let observers = self.observers.clone();
        let fanout = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Some(event) => {
                        // Snapshot the senders under a brief read-lock so we
                        // never hold the RwLock across the try_send loop.
                        let senders: Vec<mpsc::Sender<DriverEvent>> = {
                            let guard = observers.read().unwrap();
                            guard.clone()
                        };
                        for sender in &senders {
                            match sender.try_send(event.clone()) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    tracing::warn!(
                                        event_kind = event_kind_name(&event),
                                        "driver event dropped: observer queue full"
                                    );
                                    metrics::counter!("chorus_driver_events_dropped").increment(1);
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // Observer dropped its receiver. This is
                                    // not a dropped event — the subscriber
                                    // left; we prune below. Do NOT increment
                                    // the drop metric here; that counter is
                                    // reserved for Full (slow consumer).
                                }
                            }
                        }
                        // Best-effort prune once per tick. Keeps the observer
                        // list bounded without a second pass over senders on
                        // the hot path.
                        observers.write().unwrap().retain(|s| !s.is_closed());
                    }
                    None => break,
                }
                if fanout.closing.load(Ordering::SeqCst) && rx.is_empty() {
                    break;
                }
            }
            // Dispatcher exiting: drop every observer sender so subscribers'
            // `recv()` observes `None` promptly instead of waiting on the
            // `EventStreamHandle` itself to be dropped. This is how callers
            // detect a clean fan-out shutdown.
            observers.write().unwrap().clear();
        });
    }

    /// Current observer count. Test-only accessor used to verify pruning;
    /// not part of the public API.
    #[cfg(test)]
    fn observer_count(&self) -> usize {
        self.observers.read().unwrap().len()
    }
}

/// Shareable handle to a driver's event fan-out.
///
/// Cheap to clone (`Arc`). Subscribers obtain one of these from
/// [`SessionAttachment`] and call [`EventStreamHandle::subscribe`] to register a
/// listener; the Session implementation calls [`EventStreamHandle::close`]
/// when the runtime shuts down.
#[derive(Debug, Clone)]
pub struct EventStreamHandle {
    pub(crate) inner: Arc<EventFanOut>,
}

impl EventStreamHandle {
    /// Subscribe a new observer. Returns a bounded `mpsc::Receiver` (capacity
    /// [`OBSERVER_CAPACITY`], 256) that will receive every `DriverEvent`
    /// emitted by the handle's transport tasks from this moment forward.
    ///
    /// Spawns the dispatcher task on the first call (exactly once per
    /// fan-out). Back-pressure policy: if this receiver isn't drained fast
    /// enough, overflow events are dropped with a metrics bump and a warn.
    pub fn subscribe(&self) -> mpsc::Receiver<DriverEvent> {
        let (tx, rx) = mpsc::channel(OBSERVER_CAPACITY);

        // Post-exit detection: if the dispatcher has already been spawned and
        // exited (inbound_rx was taken AND closing was flipped), this
        // subscribe is too late. Skip the observer push so `tx` is dropped at
        // end of scope — the returned `rx.recv().await` then observes `None`
        // promptly instead of hanging forever on a sender that would never
        // receive events and would never be dropped. `closing + is_none()`
        // together discriminate the post-exit state from the common "running
        // dispatcher has taken the rx but not yet exited" state, where
        // `closing` is still false and a normal push is correct.
        let dispatcher_exited = self.inner.inbound_rx.lock().unwrap().is_none()
            && self.inner.closing.load(Ordering::Acquire);
        if dispatcher_exited {
            return rx;
        }

        self.inner.observers.write().unwrap().push(tx);
        self.inner.spawn_dispatcher_once();
        rx
    }

    /// Signal the fan-out dispatcher to drain its inbound queue and exit
    /// after the final event. Call this from `Session::close()` after
    /// emitting the terminal `Lifecycle::Closed` event. Idempotent.
    pub fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Request / attachment payloads
// ---------------------------------------------------------------------------

/// Attachment classification carried with a prompt.
#[derive(Debug, Clone)]
pub enum AttachmentKind {
    /// Inline image data.
    Image { mime: String },
    /// File reference (path on disk) with its MIME type.
    File { path: PathBuf, mime: String },
}

/// Raw attachment payload attached to a prompt.
#[derive(Debug, Clone)]
pub struct PromptAttachment {
    pub kind: AttachmentKind,
    pub bytes: Vec<u8>,
}

/// Prompt request sent to [`Session::prompt`].
#[derive(Debug, Clone)]
pub struct PromptReq {
    pub text: String,
    pub attachments: Vec<PromptAttachment>,
}

/// Outcome of a [`Session::cancel`] call.
#[derive(Debug, Clone)]
pub enum CancelOutcome {
    /// The in-flight run was aborted; the session remains usable.
    Aborted,
    /// The runtime replaced the session as part of cancellation.
    SessionReplaced { new_session_id: SessionId },
    /// No run was in flight when cancel was invoked.
    NotInFlight,
}

// ---------------------------------------------------------------------------
// Runtime metadata / catalog types
// ---------------------------------------------------------------------------

/// Model description returned by [`RuntimeDriver::list_models`].
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub supports_reasoning_effort: bool,
    pub default_reasoning_effort: Option<String>,
}

impl ModelInfo {
    /// Build a minimal `ModelInfo` from a bare model id string.
    /// Used by the V1 adapter when translating `Driver::list_models()`.
    pub fn from_id(id: String) -> Self {
        Self {
            display_name: id.clone(),
            id,
            supports_reasoning_effort: false,
            default_reasoning_effort: None,
        }
    }
}

/// Slash command description returned by [`RuntimeDriver::list_commands`].
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
}

/// Previously-stored session metadata returned by
/// [`RuntimeDriver::list_sessions`].
#[derive(Debug, Clone)]
pub struct StoredSessionMeta {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub cwd: Option<PathBuf>,
}

/// Aggregate result of a runtime probe.
#[derive(Debug, Clone)]
pub struct RuntimeProbe {
    pub auth: ProbeAuth,
    pub transport: TransportKind,
    pub capabilities: CapabilitySet,
}

/// Configuration needed to attach an agent to a runtime.
///
/// Built by the manager from persisted agent state. Mirrors the fields v1's
/// [`crate::agent::drivers::SpawnContext`] carries today, plus a few typed
/// extras the new drivers need.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub display_name: String,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub env_vars: Vec<AgentEnvVar>,
    pub working_directory: PathBuf,
    /// Shared bridge HTTP endpoint. All drivers connect to this endpoint for
    /// MCP transport — the per-agent stdio bridge is no longer supported.
    /// Format: `http://127.0.0.1:4321` (port from bridge discovery or config).
    pub bridge_endpoint: String,
}

/// Session intent: whether to start a new session or resume an existing one.
///
/// Used by [`RuntimeDriver::open_session`] to unify the `attach`, `new_session`,
/// and `resume_session` verbs. During migration, the default impl delegates to
/// the legacy methods.
#[derive(Debug, Clone, Default)]
pub enum SessionIntent {
    #[default]
    New,
    Resume(SessionId),
}

/// Return value of [`RuntimeDriver::open_session`].
pub struct SessionAttachment {
    pub session: Box<dyn Session>,
    pub events: EventStreamHandle,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Runtime-level factory.
///
/// One instance per runtime (Claude, Codex, Kimi, OpenCode, Fake). Session
/// lifecycle lives here: [`open_session`] opens (new or resumed) sessions,
/// each yielding a fresh [`Session`].
///
/// `'static` so driver pointers can be stored in registries; `Send + Sync`
/// because the agent manager holds them behind an `Arc`.
#[async_trait]
pub trait RuntimeDriver: Send + Sync + 'static {
    /// Identifier used in persisted agent records and logs.
    fn runtime(&self) -> AgentRuntime;

    /// Detect whether the runtime is installed, authed, and what it supports.
    async fn probe(&self) -> anyhow::Result<RuntimeProbe>;

    /// Run the runtime-specific login flow.
    async fn login(&self) -> anyhow::Result<LoginOutcome>;

    /// Enumerate previously-stored sessions for this runtime on this host.
    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>>;

    /// Enumerate the runtime's available models.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>>;

    /// Enumerate runtime-advertised slash commands (if supported).
    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>>;

    /// Open a session on an agent. Unified replacement for the legacy
    /// `attach`, `new_session`, and `resume_session` verbs.
    ///
    /// `SessionIntent::New` starts a fresh session; `SessionIntent::Resume(id)`
    /// resumes the given stored session. The returned handle is in
    /// [`AgentState::Idle`]; callers must invoke [`Session::run`] to
    /// bring it online.
    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment>;
}

/// Per-session lifecycle handle.
///
/// Represents one ACP session (or equivalent). Multiple session handles may
/// coexist for a single agent when the driver supports multiplexing; each
/// carries its own `session_id`, state, and event timeline. Consumers drive a
/// session through `run` -> `prompt` -> `cancel`/`close` transitions and
/// observe side effects on the paired [`EventStreamHandle`].
///
/// `Send` only — handles may be moved across tasks but are not required to be
/// `Sync`; serialization of concurrent access is the handle implementation's
/// responsibility (typically via an internal actor loop).
#[async_trait]
pub trait Session: Send {
    /// The agent key this session belongs to.
    fn key(&self) -> &AgentKey;

    /// The session id this handle is attached to, if one has been assigned.
    ///
    /// `None` before `run` completes (or `run` resumes without a known id).
    /// `Some` for every state downstream of `Active`.
    fn session_id(&self) -> Option<&str>;

    /// Current lifecycle state of this session.
    fn state(&self) -> AgentState;

    /// Bring the session online. Resume intent is threaded in via
    /// [`RuntimeDriver::open_session`]'s `SessionIntent`; `init_prompt`,
    /// when present, is delivered as the first prompt so some runtimes can
    /// perform session bootstrap in one turn.
    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()>;

    /// Send a prompt to the live session. Returns the [`RunId`] assigned so
    /// callers can correlate subsequent events.
    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId>;

    /// Cancel an in-flight run. Behavior depends on the runtime; see
    /// [`CancelOutcome`].
    async fn cancel(&mut self, run: RunId) -> anyhow::Result<CancelOutcome>;

    /// Shut this session down and release its resources. Does not tear down
    /// the agent's shared runtime process when other sessions remain live.
    async fn close(&mut self) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// Shared bridge pairing helper (Phase 2)
// ---------------------------------------------------------------------------

/// Request a pairing token from the shared bridge's admin endpoint.
///
/// Called by drivers when `AgentSpec.bridge_endpoint` is set. The returned
/// token is used to construct the MCP URL `{endpoint}/token/{token}/mcp`
/// which the runtime connects to for its MCP session.
///
/// Errors if the bridge is unreachable, returns a non-2xx status, or
/// omits the `token` field in the response.
pub async fn request_pairing_token(
    bridge_endpoint: &str,
    agent_key: &str,
) -> anyhow::Result<String> {
    use anyhow::Context;
    let url = format!("{}/admin/pair", bridge_endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build reqwest client")?;
    let res = client
        .post(&url)
        .json(&serde_json::json!({"agent_key": agent_key}))
        .send()
        .await
        .context("bridge unreachable")?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("bridge-pair failed: {} {}", status, body);
    }
    let json: serde_json::Value = res.json().await.context("invalid JSON from bridge")?;
    json["token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing 'token' in bridge response"))
}

// ---------------------------------------------------------------------------
// Per-driver shared scaffolding
// ---------------------------------------------------------------------------

/// Try-send a [`DriverEvent`] into a driver's inbound fan-out channel.
/// Drivers call this from their per-process / per-handle `emit` wrappers so
/// every driver behaves identically under back-pressure: a dropped event is
/// warn-logged with `agent` + `driver` fields rather than silently swallowed.
///
/// Before this helper existed, claude/codex logged `warn!` on Full while
/// kimi/opencode/fake did `let _ = try_send(...)` — the four inbound queues
/// had four different overload policies and debugging a stuck agent meant
/// guessing which driver was silent.
pub(crate) fn emit_driver_event(
    tx: &mpsc::Sender<DriverEvent>,
    event: DriverEvent,
    agent: &AgentKey,
    driver: &'static str,
) {
    if let Err(e) = tx.try_send(event) {
        tracing::warn!(agent = %agent, driver, "dropped driver event: {e}");
    }
}
/// Per-agent runtime process type. Implemented by each driver's
/// `*AgentProcess` / `*AgentCore` struct so a generic [`AgentRegistry`] can
/// cache instances of it and evict stale entries at attach time.
pub(crate) trait AgentProcess: Send + Sync + 'static {
    /// Short driver name used in registry debug logs, e.g. `"claude"`,
    /// `"codex"`, `"kimi"`, `"opencode"`.
    const DRIVER_NAME: &'static str;

    /// True if the cached process can no longer serve a new attach — the
    /// shared child has died, stdin has been closed, or the bootstrap's
    /// `close()` has marked it torn down. The registry evicts stale entries
    /// before returning them so `ensure_process` never hands callers a dead
    /// Arc.
    ///
    /// A never-spawned process is NOT stale; the bootstrap path still needs
    /// to be able to re-use a cached-but-un-started entry.
    fn is_stale(&self) -> bool;
}

/// Process-global per-driver agent registry. Replaces the four near-identical
/// `OnceLock<Mutex<HashMap<AgentKey, Arc<P>>>>` + `ensure_process` copies that
/// each driver used to carry. Instantiated once per driver as a `static`.
///
/// All methods take `&self`; the map is lazily initialized inside a `OnceLock`
/// so the type works as a `static`.
pub(crate) struct AgentRegistry<P: AgentProcess> {
    inner: std::sync::OnceLock<Mutex<std::collections::HashMap<AgentKey, Arc<P>>>>,
}

impl<P: AgentProcess> AgentRegistry<P> {
    pub(crate) const fn new() -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
        }
    }

    fn map(&self) -> &Mutex<std::collections::HashMap<AgentKey, Arc<P>>> {
        self.inner
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }

    /// Return the cached process for `key`, building a fresh one via
    /// `factory` if the slot is empty. A cached-but-stale entry is evicted
    /// and `factory` runs anew. The lookup, eviction, and insert all happen
    /// under the same critical section so two concurrent `attach`es on the
    /// same key observe each other.
    pub(crate) fn get_or_init<F>(&self, key: &AgentKey, factory: F) -> Arc<P>
    where
        F: FnOnce() -> Arc<P>,
    {
        let mut guard = self.map().lock().unwrap();
        if let Some(existing) = guard.get(key) {
            if existing.is_stale() {
                tracing::debug!(
                    agent = %key,
                    driver = P::DRIVER_NAME,
                    "evicting stale agent process before re-attach",
                );
                guard.remove(key);
            } else {
                return Arc::clone(existing);
            }
        }
        let fresh = factory();
        guard.insert(key.clone(), Arc::clone(&fresh));
        fresh
    }

    /// Return the cached process for `key`, evicting it first if stale.
    /// Used by drivers whose attach path constructs the process inline
    /// and then calls [`AgentRegistry::insert`] separately (kimi).
    pub(crate) fn get_or_evict_stale(&self, key: &AgentKey) -> Option<Arc<P>> {
        let mut guard = self.map().lock().unwrap();
        if let Some(existing) = guard.get(key) {
            if existing.is_stale() {
                tracing::debug!(
                    agent = %key,
                    driver = P::DRIVER_NAME,
                    "evicting stale agent process from registry",
                );
                guard.remove(key);
                return None;
            }
            return Some(Arc::clone(existing));
        }
        None
    }

    /// Raw read — does NOT evict stale entries. Used by tests that inspect
    /// registry state without mutating it, and by driver code that needs the
    /// `Arc` handle itself rather than building or evicting one.
    #[allow(dead_code)]
    pub(crate) fn get(&self, key: &AgentKey) -> Option<Arc<P>> {
        self.map().lock().unwrap().get(key).cloned()
    }

    /// Raw insert. Overwrites any existing entry without a stale check.
    pub(crate) fn insert(&self, key: AgentKey, proc: Arc<P>) {
        self.map().lock().unwrap().insert(key, proc);
    }

    /// Raw remove.
    pub(crate) fn remove(&self, key: &AgentKey) {
        self.map().lock().unwrap().remove(key);
    }

    /// Escape hatch: lock the underlying map for multi-step operations
    /// (e.g., `Arc::strong_count` checks in tests). Most callers should
    /// use the per-method helpers above.
    #[allow(dead_code)]
    pub(crate) fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<AgentKey, Arc<P>>> {
        self.map().lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::timeout;

    // ---- SessionIntent ----

    #[test]
    fn session_intent_default_is_new() {
        assert!(matches!(SessionIntent::default(), SessionIntent::New));
    }

    #[test]
    fn session_intent_resume_carries_id() {
        match SessionIntent::Resume("sess_abc".to_string()) {
            SessionIntent::Resume(id) => assert_eq!(id, "sess_abc"),
            _ => panic!("expected Resume"),
        }
    }

    /// Minimal `DriverEvent` helper used across tests. Uses `Lifecycle` with
    /// a cheap-to-clone `Idle` state so the fan-out dispatcher isn't
    /// sensitive to clone cost.
    fn test_event(key: &str) -> DriverEvent {
        DriverEvent::Lifecycle {
            key: key.to_string(),
            state: AgentState::Idle,
        }
    }

    /// Extract the key out of a `Lifecycle` event for assertion convenience.
    fn lifecycle_key(ev: &DriverEvent) -> &str {
        match ev {
            DriverEvent::Lifecycle { key, .. } => key.as_str(),
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn first_subscribe_spawns_dispatcher() {
        let (stream, tx) = EventFanOut::new();
        let mut rx = stream.subscribe();

        tx.send(test_event("a1")).await.unwrap();

        let got = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("dispatcher did not forward event within 500ms")
            .expect("observer channel closed unexpectedly");
        assert_eq!(lifecycle_key(&got), "a1");
    }

    #[tokio::test]
    async fn second_subscribe_does_not_respawn_dispatcher() {
        let (stream, tx) = EventFanOut::new();
        let mut rx1 = stream.subscribe();
        // If spawn_dispatcher_once misbehaved and tried to take the inbound
        // receiver again, the second call would race on the Mutex<Option<_>>
        // and find None — no panic, but also no second dispatcher stealing
        // events. Both observers getting the same event is the positive
        // signal that exactly one dispatcher is live.
        let mut rx2 = stream.subscribe();

        tx.send(test_event("b1")).await.unwrap();

        let got1 = timeout(Duration::from_millis(500), rx1.recv())
            .await
            .expect("rx1 timeout")
            .expect("rx1 closed");
        let got2 = timeout(Duration::from_millis(500), rx2.recv())
            .await
            .expect("rx2 timeout")
            .expect("rx2 closed");
        assert_eq!(lifecycle_key(&got1), "b1");
        assert_eq!(lifecycle_key(&got2), "b1");
    }

    #[tokio::test]
    async fn other_observers_still_receive_when_one_is_slow() {
        // THE key back-pressure invariant: one slow observer must not stall
        // the dispatcher for everyone else.
        let (stream, tx) = EventFanOut::new();
        let mut fast = stream.subscribe();
        let _slow = stream.subscribe(); // never drained

        // 260 events — 4 more than the observer queue depth.
        for i in 0..260 {
            tx.send(test_event(&format!("e{i}"))).await.unwrap();
        }

        // Fast observer should receive all 260 events.
        let mut fast_count = 0usize;
        while let Ok(Some(_)) = timeout(Duration::from_millis(500), fast.recv()).await {
            fast_count += 1;
            if fast_count == 260 {
                break;
            }
        }
        assert_eq!(
            fast_count, 260,
            "fast observer should receive every event despite slow peer",
        );
    }

    #[tokio::test]
    async fn observer_queue_full_drops_event() {
        // Proves drop-on-full behavior by contrast: fill a slow observer's
        // 256-deep queue and show a fast peer still sees every event past
        // that point.
        let (stream, tx) = EventFanOut::new();
        let mut slow = stream.subscribe();
        let mut fast = stream.subscribe();

        // Push OBSERVER_CAPACITY + 10 events so the slow queue overflows by
        // 10; the fast observer drains concurrently via this task.
        let total = OBSERVER_CAPACITY + 10;
        for i in 0..total {
            tx.send(test_event(&format!("x{i}"))).await.unwrap();
        }

        // Drain fast observer completely. It should see all `total` events:
        // drop-on-full on the slow observer must not starve the fast one.
        let mut fast_count = 0usize;
        while fast_count < total {
            match timeout(Duration::from_millis(500), fast.recv()).await {
                Ok(Some(_)) => fast_count += 1,
                Ok(None) => panic!("fast observer channel closed early"),
                Err(_) => panic!("fast observer timed out at {fast_count}/{total}",),
            }
        }
        assert_eq!(fast_count, total);

        // Slow observer's queue has capacity OBSERVER_CAPACITY; everything
        // past that was dropped. Drain to verify the bound holds.
        let mut slow_count = 0usize;
        while let Ok(Some(_)) = timeout(Duration::from_millis(50), slow.recv()).await {
            slow_count += 1;
            if slow_count > OBSERVER_CAPACITY {
                break;
            }
        }
        assert!(
            slow_count <= OBSERVER_CAPACITY,
            "slow observer received {slow_count} events, exceeds cap {OBSERVER_CAPACITY}",
        );
    }

    #[tokio::test]
    async fn closed_observer_is_pruned() {
        let (stream, tx) = EventFanOut::new();
        let fanout = stream.inner.clone();

        // Subscribe, then immediately drop the receiver.
        drop(stream.subscribe());
        assert_eq!(fanout.observer_count(), 1);

        // Send an event to drive the dispatcher into its post-send prune.
        tx.send(test_event("prune-1")).await.unwrap();

        // Give the dispatcher a tick to prune. Poll the accessor rather than
        // sleeping a fixed amount — keeps the test fast when it's already done.
        let pruned = timeout(Duration::from_secs(2), async {
            loop {
                if fanout.observer_count() == 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false);
        assert!(pruned, "closed observer was not pruned from the registry");
    }

    #[tokio::test]
    async fn close_signal_drains_and_exits() {
        let (stream, tx) = EventFanOut::new();
        let mut rx = stream.subscribe();

        for i in 0..3 {
            tx.send(test_event(&format!("c{i}"))).await.unwrap();
        }

        // Flip the closing flag, then drop the sender so recv() returns None.
        stream.close();
        drop(tx);

        // We should receive all three events before seeing the stream close.
        for i in 0..3 {
            let got = timeout(Duration::from_millis(500), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timeout waiting for event c{i}"))
                .unwrap_or_else(|| panic!("channel closed before event c{i}"));
            assert_eq!(lifecycle_key(&got), format!("c{i}"));
        }

        // After the dispatcher drains and exits, every observer sender is
        // dropped, so the next recv() observes None.
        let end = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("observer channel did not close after drain");
        assert!(end.is_none(), "expected None after drain, got {end:?}");
    }

    #[tokio::test]
    async fn subscribe_after_close_returns_none_promptly() {
        // Regression guard: once close() has fired AND the dispatcher has
        // exited, a late subscribe() must not register an observer — its
        // receiver would otherwise hang forever on a sender nothing feeds
        // and nothing drops. Expected behavior is that the late receiver
        // sees `None` promptly (tx dropped inside subscribe()).
        let (stream, tx) = EventFanOut::new();

        // First subscribe spawns the dispatcher (inbound_rx taken).
        let _rx_original = stream.subscribe();

        // Close + drop inbound sender so the dispatcher's recv() returns None
        // and the task exits.
        stream.close();
        drop(tx);

        // Give the dispatcher a brief moment to actually exit. The test above
        // (`close_signal_drains_and_exits`) already asserts this happens; we
        // just need to be past that point here.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Late subscribe — after dispatcher has exited. Must return a
        // receiver that sees None promptly, not one that hangs forever.
        let mut rx_late = stream.subscribe();
        let end = timeout(Duration::from_millis(500), rx_late.recv())
            .await
            .expect("late subscribe receiver hung instead of seeing None");
        assert!(
            end.is_none(),
            "late subscribe should observe None after dispatcher exit, got {end:?}",
        );
    }

    // ---- request_pairing_token ----

    async fn spawn_mock_bridge(
        response: axum::response::Response,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::post;
        use axum::Router;
        use std::sync::Mutex as StdMutex;

        let shared = Arc::new(StdMutex::new(Some(response)));
        let app = Router::new().route(
            "/admin/pair",
            post(move || {
                let shared = shared.clone();
                async move {
                    shared
                        .lock()
                        .unwrap()
                        .take()
                        .expect("mock bridge only answers once")
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Give the listener a tick to be ready.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        (url, handle)
    }

    #[tokio::test]
    async fn request_pairing_token_parses_success_response() {
        let response = axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({"token": "tok-123"}).to_string(),
            ))
            .unwrap();
        let (url, handle) = spawn_mock_bridge(response).await;

        let token = request_pairing_token(&url, "agent-abc").await.unwrap();
        assert_eq!(token, "tok-123");
        handle.abort();
    }

    #[tokio::test]
    async fn request_pairing_token_surfaces_non_2xx() {
        let response = axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from("boom"))
            .unwrap();
        let (url, handle) = spawn_mock_bridge(response).await;

        let err = request_pairing_token(&url, "agent-abc")
            .await
            .expect_err("non-2xx should bubble up");
        let msg = format!("{err:#}");
        assert!(msg.contains("bridge-pair failed"), "got: {msg}");
        handle.abort();
    }

    #[tokio::test]
    async fn request_pairing_token_errors_on_missing_field() {
        let response = axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({"wrong_field": "x"}).to_string(),
            ))
            .unwrap();
        let (url, handle) = spawn_mock_bridge(response).await;

        let err = request_pairing_token(&url, "agent-abc")
            .await
            .expect_err("missing token field should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("missing 'token'"), "got: {msg}");
        handle.abort();
    }
}
