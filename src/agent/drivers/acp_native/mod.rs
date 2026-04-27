//! Shared base for ACP-native drivers (gemini, kimi, opencode).
//!
//! Each driver supplies a `&'static AcpDriverConfig` describing the
//! per-runtime variation: how to spawn the child process, the JSON shape of
//! the MCP servers in `session/new` params, whether `session/load` re-binds
//! MCP servers, whether to send an `initialized` notification after the
//! `initialize` response, and whether to prepend a per-runtime "standing
//! prompt" to the first user turn.
//!
//! Everything else — the reader loop, response routing, session lifecycle,
//! cancel/close, the closed_emitted race guard — lives here once.
//!
//! Behavior is preserved bit-for-bit relative to the per-driver
//! implementations this module replaces. ACP spec compliance gaps (cancel
//! notification, stopReason parsing, session/close RPC, capability checking,
//! session/resume, history replay drain, rich update variants, MCP transport
//! negotiation) are tracked as follow-up work — see the plan at
//! `docs/plans/2026-04-27-acp-native-driver-unification-plan.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::AgentRuntime;

use super::{
    AgentKey, AgentProcess, AgentRegistry, AgentSpec, DriverEvent, EventFanOut, SessionAttachment,
    SessionIntent,
};

mod core;
mod handle;
mod reader;
mod state;

#[cfg(test)]
mod tests;

pub(crate) use self::core::AcpNativeCore;
pub(crate) use self::handle::AcpNativeHandle;

// ---------------------------------------------------------------------------
// Configuration surface
// ---------------------------------------------------------------------------

/// When and how the first user prompt is delivered after `session/new`
/// completes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum InitPromptStrategy {
    /// Send the prompt immediately after `session/new`. Used by gemini and
    /// kimi.
    Immediate,
    /// Wait for the caller to invoke [`super::Session::prompt`] explicitly.
    /// Used by opencode (deferred init prompt — landed in PR2).
    #[allow(dead_code)]
    Deferred,
}

/// Outcome of [`AcpDriverConfig::spawn_child`]. Owns the spawned `Child` plus
/// any optional payload that needs to ride along — currently nothing else,
/// but kept as a struct so future fields (e.g. spec-specific paths the reader
/// needs) don't change every driver's signature.
pub(crate) struct SpawnedChild {
    pub child: std::process::Child,
}

/// Boxed future returned by [`AcpDriverConfig::spawn_child`]. Async fn
/// pointers can't be expressed directly in Rust today; the boxed-future
/// pattern is the standard workaround.
pub(crate) type SpawnFut =
    Pin<Box<dyn Future<Output = anyhow::Result<SpawnedChild>> + Send + 'static>>;

/// Per-runtime variation surface. Each driver owns a `&'static
/// AcpDriverConfig` that the shared core / handle / reader read from.
///
/// The fields are deliberately concrete (function pointers, bools,
/// `Option<&'static str>`) rather than a trait. With three runtimes and no
/// hot-path dispatch concerns, a struct is simpler than a trait + generics
/// and keeps the door open to swap function pointers for closures later if
/// a driver legitimately needs context capture.
pub(crate) struct AcpDriverConfig {
    /// Short human-readable driver name. Used in event-emission warn logs
    /// and tracing context. e.g. `"kimi"`, `"gemini"`, `"opencode"`.
    pub name: &'static str,

    /// Runtime tag exposed via [`super::RuntimeDriver::runtime`].
    pub runtime: AgentRuntime,

    /// First-turn prompt delivery strategy. See [`InitPromptStrategy`].
    pub init_prompt_strategy: InitPromptStrategy,

    /// Optional notification sent on stdin after the `initialize` response
    /// arrives. Some agents (gemini) require an `initialized` JSON-RPC
    /// notification post-handshake; most do not. `None` skips the send.
    pub initialized_notification_payload: Option<&'static str>,

    /// Whether `session/load` params should include the populated MCP
    /// servers list (kimi) or an empty array (gemini). The ACP spec leaves
    /// this implementation-defined; we preserve each runtime's existing
    /// behavior bit-for-bit.
    pub session_load_includes_mcp: bool,

    /// Whether to emit a `Lifecycle::Starting` event at the top of `run`,
    /// before `ensure_started`. Kimi did this; gemini did not — preserving
    /// both behaviors.
    pub emit_starting_lifecycle: bool,

    /// Build the `mcpServers` JSON array embedded in `session/new` params.
    /// Each runtime has its own preferred shape (header keys,
    /// transport-type field naming).
    pub build_session_new_mcp_servers: fn(bridge_endpoint: &str, agent_key: &str) -> Value,

    /// Optional builder for a per-runtime "standing prompt" prepended to
    /// the first user turn. When `Some(_)`, `run_inner` always sends a
    /// prompt: either standing-only (when `init_prompt` is None) or
    /// `standing + "---" + init_prompt` (when present). When `None`, the
    /// first turn fires only if the caller passed `init_prompt: Some(_)`.
    pub build_first_prompt_prefix: Option<fn(&AgentSpec) -> String>,

    /// Spawn the runtime's child process. The driver's implementation
    /// performs all per-runtime setup (write config files, build the
    /// command, spawn) and returns the live [`std::process::Child`].
    pub spawn_child: fn(spec: Arc<AgentSpec>, key: AgentKey) -> SpawnFut,

    /// Per-driver static registry of live cores. Each driver owns its own
    /// `AgentRegistry<AcpNativeCore>` so kimi keys never collide with
    /// gemini keys. The shared core / close path looks up the right
    /// registry through this fn pointer.
    pub registry: fn() -> &'static AgentRegistry<AcpNativeCore>,
}

// ---------------------------------------------------------------------------
// Shared open_session helper
// ---------------------------------------------------------------------------

/// Common `RuntimeDriver::open_session` body. Each driver's trait impl
/// delegates here so the registry lookup, fresh-core construction, and
/// `SessionAttachment` packaging live in one place.
///
/// The driver provides only the `&'static AcpDriverConfig` plus the call
/// arguments. Behavior is preserved bit-for-bit relative to the kimi /
/// gemini per-driver `open_session` implementations.
pub(crate) async fn open_session(
    cfg: &'static AcpDriverConfig,
    key: AgentKey,
    spec: AgentSpec,
    intent: SessionIntent,
) -> anyhow::Result<SessionAttachment> {
    let registry = (cfg.registry)();
    let core = if let Some(existing) = registry.get_or_evict_stale(&key) {
        existing
    } else {
        let (events, event_tx) = EventFanOut::new();
        let fresh = AcpNativeCore::new(cfg, key.clone(), spec, events, event_tx);
        registry.insert(key.clone(), fresh.clone());
        fresh
    };
    let events = core.events.clone();
    let preassigned = match intent {
        SessionIntent::New => None,
        SessionIntent::Resume(id) => Some(id),
    };
    let handle = AcpNativeHandle::new(core, preassigned);
    Ok(SessionAttachment {
        session: Box::new(handle),
        events,
    })
}

// ---------------------------------------------------------------------------
// Internal helper: emit a DriverEvent through the core's fan-out channel.
// ---------------------------------------------------------------------------

/// Per-core / per-handle event emit. Wraps [`super::emit_driver_event`] with
/// the driver name read from `cfg`, so log lines retain per-runtime
/// distinction even though the underlying core type is shared.
pub(crate) fn emit_through(
    cfg: &AcpDriverConfig,
    event_tx: &mpsc::Sender<DriverEvent>,
    event: DriverEvent,
    key: &AgentKey,
) {
    super::emit_driver_event(event_tx, event, key, cfg.name);
}

// ---------------------------------------------------------------------------
// AgentProcess trait impl for the shared core. DRIVER_NAME is generic
// because one type backs three drivers; per-driver name flows through
// emit_through above.
// ---------------------------------------------------------------------------

impl AgentProcess for AcpNativeCore {
    const DRIVER_NAME: &'static str = "acp_native";

    fn is_stale(&self) -> bool {
        AcpNativeCore::is_stale_impl(self)
    }
}
