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

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    AcpAdapter,
    AcpNative,
    StreamJson,
    HttpAppServer,
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
    Thinking { text: String },
    Text { text: String },
    /// Transport layers MUST coalesce ACP's deferred `tool_call_update` frames
    /// into this variant before emitting; callers never see partial tool-call
    /// input.
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    ToolResult { content: String },
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
// Event stream / fan-out (placeholder — filled in Task 2)
// ---------------------------------------------------------------------------

/// Broadcast dispatcher placeholder.
///
/// The actual channels, subscriber registry, and backpressure policy land in
/// Task 2. Defined here so [`EventStreamHandle`] and [`AttachResult`] have a
/// real type to hold. `pub` because it's reachable through the public handle.
#[derive(Debug, Default)]
pub struct EventFanOut {
    // Intentionally empty. Task 2 adds subscriber channels + dispatch loop.
}

/// Shareable handle to a driver's event fan-out.
///
/// Cheap to clone (`Arc`). Subscribers obtain one of these from
/// [`AttachResult`] and use it to register listeners once the fan-out API
/// lands in Task 2.
#[derive(Debug, Clone)]
pub struct EventStreamHandle {
    // Task 2 adds the subscribe API that reads this; until then the field is
    // only written in constructors.
    #[allow(dead_code)]
    pub(crate) inner: Arc<EventFanOut>,
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
