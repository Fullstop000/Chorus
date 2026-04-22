//! Phase 4 Task 4.1 regression: stopping or sleeping an agent must NOT clear
//! the persisted `session_id`. Session lifetime is independent of process
//! lifetime — the next `start_agent` must be able to resume the prior
//! conversation. Clearing the session only happens via the explicit
//! `reset_session` / `full_reset` restart modes on the agents handler.
//!
//! We bypass the full driver/bridge path by injecting a pre-built
//! `FakeHandle` into the manager via the `inject_session_for_test` seam.
//! That puts `stop_agent` / `sleep_agent` on the branch that used to call
//! `update_agent_session(agent_name, None)`. After the branch runs, the
//! row's `session_id` must still equal what we seeded.
//!
//! If someone re-introduces the clearing line, these tests fail.

use std::sync::Arc;

use chorus::agent::drivers::fake::FakeHandle;
use chorus::agent::drivers::{EventFanOut, ProcessState};
use chorus::agent::manager::AgentManager;
use chorus::store::{AgentRecordUpsert, Store};
use tempfile::tempdir;

fn seed_agent_with_session(store: &Store, name: &str, session_id: &str) -> String {
    store
        .create_agent_record(&AgentRecordUpsert {
            name,
            display_name: "Session Persistence Bot",
            description: Some("Phase 4 session-persistence regression"),
            system_prompt: None,
            runtime: "codex",
            model: "gpt-fake",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    let agent_id = store.get_agent(name).unwrap().unwrap().id;
    store
        .record_session(&agent_id, session_id, "codex")
        .unwrap();
    agent_id
}

#[tokio::test]
async fn stop_agent_preserves_session_id() {
    let dir = tempdir().unwrap();
    let store = Arc::new(
        Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap(),
    );

    let name = "persist-bot";
    let seeded_session = "sess-stop-123";
    let agent_id = seed_agent_with_session(&store, name, seeded_session);

    let manager = AgentManager::new_for_test(store.clone(), dir.path().to_path_buf());

    // Inject an Active handle so stop_agent's `if let Some(mut agent) = ...`
    // branch runs — that's the branch that used to clear session_id.
    let (events, event_tx) = EventFanOut::new();
    let handle = FakeHandle::new(name.to_string(), events, event_tx).with_state(
        ProcessState::Active {
            session_id: seeded_session.to_string(),
        },
    );
    manager
        .inject_session_for_test(name, Box::new(handle))
        .await;

    manager.stop_agent(name).await.unwrap();

    let after = store.get_active_session(&agent_id).unwrap();
    assert_eq!(
        after.as_ref().map(|s| s.session_id.as_str()),
        Some(seeded_session),
        "stop_agent must not clear the persisted session — the next \
         start_agent needs it to issue a Resume intent. If this assertion \
         fails, someone re-introduced session-clearing in the stop path.",
    );
}

#[tokio::test]
async fn sleep_agent_preserves_session_id() {
    let dir = tempdir().unwrap();
    let store = Arc::new(
        Store::open(dir.path().join("chorus.db").to_str().unwrap()).unwrap(),
    );

    let name = "sleepy-bot";
    let seeded_session = "sess-sleep-456";
    let agent_id = seed_agent_with_session(&store, name, seeded_session);

    let manager = AgentManager::new_for_test(store.clone(), dir.path().to_path_buf());

    let (events, event_tx) = EventFanOut::new();
    let handle = FakeHandle::new(name.to_string(), events, event_tx).with_state(
        ProcessState::Active {
            session_id: seeded_session.to_string(),
        },
    );
    manager
        .inject_session_for_test(name, Box::new(handle))
        .await;

    manager.sleep_agent(name).await.unwrap();

    let after = store.get_active_session(&agent_id).unwrap();
    assert_eq!(
        after.as_ref().map(|s| s.session_id.as_str()),
        Some(seeded_session),
        "sleep_agent must not clear the persisted session — drivers like \
         claude-code and codex keep server-side conversation state across \
         process restarts, and resuming depends on this id surviving sleep.",
    );
}
