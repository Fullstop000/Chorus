//! User-facing agent status derived from `ProcessState` + manager presence.
//!
//! Four values, one source of truth. Never persisted.

use crate::agent::drivers::ProcessState;
use serde::{Deserialize, Serialize};

/// What a user sees in the sidebar / agent list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Process alive, prompt currently in flight (or starting up).
    Working,
    /// Process alive, idle, can accept work with low latency.
    Ready,
    /// No process. Will wake on the next incoming message.
    Asleep,
    /// Last process attempt failed; user attention required.
    Failed,
}

/// Derive the user-facing `Status` from the optional `ProcessState`
/// reported by the in-memory `AgentManager`.
///
/// `None` means "no entry in the manager HashMap" — agent has no process.
pub fn derive_status(process_state: Option<&ProcessState>) -> Status {
    match process_state {
        None => Status::Asleep,
        // Idle is a transient construction state — the handle exists but
        // open_session was never called. Users shouldn't observe Idle in
        // normal flow; treating it as Asleep ("send a message to wake it")
        // is the correct user-facing behavior.
        Some(ProcessState::Closed) | Some(ProcessState::Idle) => Status::Asleep,
        Some(ProcessState::Starting) => Status::Working,
        Some(ProcessState::PromptInFlight { .. }) => Status::Working,
        Some(ProcessState::Active { .. }) => Status::Ready,
        Some(ProcessState::Failed(_)) => Status::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::drivers::AgentError;

    #[test]
    fn no_process_means_asleep() {
        assert_eq!(derive_status(None), Status::Asleep);
    }

    #[test]
    fn closed_and_idle_collapse_to_asleep() {
        assert_eq!(derive_status(Some(&ProcessState::Closed)), Status::Asleep);
        assert_eq!(derive_status(Some(&ProcessState::Idle)), Status::Asleep);
    }

    #[test]
    fn starting_and_in_flight_are_working() {
        assert_eq!(derive_status(Some(&ProcessState::Starting)), Status::Working);
        assert_eq!(
            derive_status(Some(&ProcessState::PromptInFlight {
                run_id: uuid::Uuid::nil(),
                session_id: "s".into(),
            })),
            Status::Working
        );
    }

    #[test]
    fn active_is_ready() {
        assert_eq!(
            derive_status(Some(&ProcessState::Active { session_id: "s".into() })),
            Status::Ready
        );
    }

    #[test]
    fn failed_carries_through() {
        assert_eq!(
            derive_status(Some(&ProcessState::Failed(AgentError::Timeout))),
            Status::Failed
        );
    }

    #[test]
    fn serialises_snake_case() {
        let s = serde_json::to_string(&Status::Working).unwrap();
        assert_eq!(s, "\"working\"");
    }
}
