//! Shared reader-task state.
//!
//! Populated during [`super::AcpNativeCore::spawn_and_initialize`], consumed
//! by the stdout task. Mirrors the per-driver `SharedReaderState` types
//! kimi.rs and gemini.rs used to carry — single source of truth now.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::oneshot;

use super::super::acp_protocol::{self, ToolCallAccumulator};
use super::super::{ProcessState, RunId};

/// Reader-task state. One instance per shared core. Wrapped in
/// `Arc<Mutex<_>>` and shared between the spawn path, the reader loop, and
/// every handle's send/cancel/close paths.
pub(crate) struct SharedReaderState {
    /// Handshake phase for the very first `initialize` response. After that
    /// response arrives the phase flips to Active and all subsequent
    /// session/new, session/load, and prompt responses route through
    /// `pending` directly.
    pub phase: acp_protocol::AcpPhase,

    /// Per-session state keyed by ACP session id.
    pub sessions: HashMap<String, SessionState>,

    /// In-flight JSON-RPC requests keyed by id. Responses are routed
    /// through this map instead of `acp_protocol::parse_line`'s hardcoded
    /// id dispatch (which otherwise buckets id>=3 as PromptResponse and
    /// would misclassify a second `session/new` at id>=3 as a prompt).
    pub pending: HashMap<u64, PendingRequest>,

    /// Set to true once a `Lifecycle { Closed }` has been emitted for this
    /// agent (either by `close()` on a handle, or the reader EOF path).
    /// Guards against duplicate emissions when both fire.
    pub closed_emitted: Arc<AtomicBool>,

    /// Optional notification to send after the `initialize` response
    /// arrives. Set by [`super::AcpNativeCore::spawn_and_initialize`] from
    /// `cfg.initialized_notification_payload`. The reader takes this
    /// `Option` on init-response receipt — `None` for kimi/opencode,
    /// `Some(json)` for gemini.
    pub initialized_notification: Option<String>,
}

/// Per-session state. Each ACP session has its own lifecycle and tool-call
/// accumulator so interleaved `session/update` notifications from different
/// sessions don't cross-contaminate.
pub(crate) struct SessionState {
    pub state: ProcessState,
    pub run_id: Option<RunId>,
    pub tool_accumulator: ToolCallAccumulator,
}

impl SessionState {
    pub fn new(session_id: &str) -> Self {
        Self {
            state: ProcessState::Active {
                session_id: session_id.to_string(),
            },
            run_id: None,
            tool_accumulator: ToolCallAccumulator::new(),
        }
    }
}

/// What an in-flight JSON-RPC request is waiting for. When the matching
/// response arrives the reader task looks up the entry and either completes
/// a oneshot (for `session/new`, `session/load`, `initialize`) or drives
/// prompt bookkeeping.
pub(crate) enum PendingRequest {
    /// `initialize` response. Only used for the first `initialize` request;
    /// the reader flips `phase` to Active on arrival.
    Init,
    /// `session/new` response — carries a oneshot that receives the minted
    /// session id (or an error).
    SessionNew {
        responder: oneshot::Sender<Result<String, String>>,
    },
    /// `session/load` response — carries the id the caller requested (to
    /// fall back to if the runtime omits sessionId in the response, which
    /// kimi does) plus the responder.
    SessionLoad {
        expected_session_id: String,
        responder: oneshot::Sender<Result<String, String>>,
    },
    /// `session/prompt` response. On arrival the reader flushes the
    /// session's tool-call accumulator, emits TurnEnd + Completed, and
    /// flips the session's state back to Active.
    Prompt { session_id: String, run_id: RunId },
}
