//! v2 driver abstraction.
//!
//! This module contains the trait and type scaffolding for the next-generation
//! driver layer. The v1 [`crate::agent::drivers::Driver`] trait continues to
//! back every runtime in production; v2 will be wired in behind a runtime
//! toggle in a later task, at which point concrete implementations land here.
//!
//! Task 1 scope is intentionally narrow: publish the trait shape and the
//! supporting data types so later tasks (EventFanOut, per-runtime drivers,
//! adapter glue) can compile against a stable surface.
//!
//! Nothing in this module is wired into the runtime yet. Adding concrete
//! drivers or invoking these traits from `AgentManager` will happen in
//! follow-up tasks.

pub mod acp_protocol;
pub mod claude;
pub mod codex;
pub mod fake;
pub mod kimi;
pub mod opencode;
pub mod v1_adapter;

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
/// newtype in v2 — the v1 code already treats this as a `String`.
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub session_id: SessionId,
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
    /// A single output item from an in-flight run.
    Output {
        key: AgentKey,
        run_id: RunId,
        item: AgentEventItem,
    },
    /// A run completed successfully.
    Completed {
        key: AgentKey,
        run_id: RunId,
        result: RunResult,
    },
    /// A run failed.
    Failed {
        key: AgentKey,
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
/// [`AttachResult`] and call [`EventStreamHandle::subscribe`] to register a
/// listener; the AgentHandle implementation calls [`EventStreamHandle::close`]
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
    /// after the final event. Call this from `AgentHandle::close()` after
    /// emitting the terminal `Lifecycle::Closed` event. Idempotent.
    pub fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Request / attachment payloads
// ---------------------------------------------------------------------------

/// Options passed to [`AgentHandle::start`].
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// If set, the driver should resume the given session instead of minting
    /// a new one. None means "start fresh via `new_session`".
    pub resume_session_id: Option<SessionId>,
}

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

/// Prompt request sent to [`AgentHandle::prompt`].
#[derive(Debug, Clone)]
pub struct PromptReq {
    pub text: String,
    pub attachments: Vec<PromptAttachment>,
}

/// Outcome of a [`AgentHandle::cancel`] call.
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
    pub bridge_binary: String,
    pub server_url: String,
}

/// Return value of [`RuntimeDriver::attach`].
pub struct AttachResult {
    pub handle: Box<dyn AgentHandle>,
    pub events: EventStreamHandle,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Runtime-level factory.
///
/// One instance per runtime (Claude, Codex, Kimi, OpenCode, Fake). Stateless
/// with respect to individual agents — it probes the host, lists catalog
/// data, and mints per-agent [`AgentHandle`]s on demand.
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

    /// Build a per-agent handle and its event stream for the given key/spec.
    ///
    /// The returned handle is in [`AgentState::Idle`]; callers must invoke
    /// `start` to bring it online.
    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult>;
}

/// Per-agent lifecycle handle.
///
/// Owns exactly one runtime process (or equivalent connection). Consumers
/// drive it through `start` -> `prompt` -> `cancel`/`close` transitions and
/// observe side effects on the paired [`EventStreamHandle`].
///
/// `Send` only — handles may be moved across tasks but are not required to be
/// `Sync`; serialization of concurrent access is the handle implementation's
/// responsibility (typically via an internal actor loop landing in Task 11).
#[async_trait]
pub trait AgentHandle: Send {
    /// The agent key this handle was attached under.
    fn key(&self) -> &AgentKey;

    /// Current lifecycle state.
    fn state(&self) -> AgentState;

    /// Bring the runtime online.
    ///
    /// If `opts.resume_session_id` is set, resumes that session; otherwise
    /// starts fresh. `init_prompt`, when present, is delivered as the first
    /// prompt so some runtimes can perform session bootstrap in one turn.
    async fn start(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()>;

    /// Start a new session on an already-started handle, replacing the
    /// currently attached session.
    async fn new_session(&mut self) -> anyhow::Result<SessionId>;

    /// Resume a previously-stored session on an already-started handle.
    async fn resume_session(&mut self, id: SessionId) -> anyhow::Result<()>;

    /// Send a prompt to the live session. Returns the [`RunId`] assigned so
    /// callers can correlate subsequent events.
    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId>;

    /// Cancel an in-flight run. Behavior depends on the runtime; see
    /// [`CancelOutcome`].
    async fn cancel(&mut self, run: RunId) -> anyhow::Result<CancelOutcome>;

    /// Shut the runtime down and release all resources.
    async fn close(&mut self) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

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
}
