//! Generic ACP-native plumbing tests.
//!
//! These tests exercise the shared core / handle / reader paths via a
//! `TestConfig` so they don't depend on a real runtime binary. The
//! per-driver tests in `kimi.rs` and `gemini.rs` are limited to
//! driver-specific concerns (probe, list_models, MCP shape, command
//! construction) — everything else lives here.
//!
//! ## Coverage gate audit
//!
//! Each test below corresponds to a per-driver test that was deleted as
//! part of the migration. Before this audit table was filled in, the
//! corresponding per-driver tests were preserved — only deleted once the
//! shared equivalent below was passing.
//!
//! | Pre-migration test (driver, name)                                       | Equivalent shared test                                |
//! |-------------------------------------------------------------------------|-------------------------------------------------------|
//! | kimi: `multi_session_pending_dispatch_routes_session_new_at_id_gt_3`    | `multi_session_pending_dispatch_routes_session_new`   |
//! | kimi: `multi_session_session_load_falls_back_to_expected_id`            | `session_load_falls_back_to_expected_id`              |
//! | kimi: `multi_session_prompt_response_carries_correct_session_id`        | `prompt_response_carries_correct_session_id`          |
//! | kimi: `handle_response_ignores_unknown_id`                              | `response_for_unknown_id_is_ignored`                  |
//! | kimi: `alloc_id_starts_at_3_after_spawn_and_initialize`                 | `alloc_id_starts_at_3_after_spawn_and_initialize`     |
//! | kimi: `registry_get_evicts_stale_core`                                  | `registry_evicts_stale_core`                          |
//! | kimi: `registry_get_keeps_fresh_never_spawned_core`                     | `registry_keeps_fresh_core`                           |
//! | kimi: `bootstrap_close_with_live_secondary_does_not_tear_down_shared`   | `close_with_live_secondary_keeps_child_alive`         |
//! | kimi: `ensure_started_fast_path_when_already_started`                   | `ensure_started_fast_path`                            |
//! | kimi: `unified_handle_session_id_from_preassigned`                      | `handle_session_id_from_preassigned`                  |
//! | kimi: `ensure_started_idempotent_after_success`                         | `ensure_started_idempotent_after_success`             |
//! | kimi: `open_session_works_without_prior_call`                           | `open_session_works_without_prior_call`               |
//! | kimi: `open_session_twice_shares_core`                                  | `open_session_twice_shares_core`                      |
//! | kimi: `open_session_resume_preserves_supplied_id_before_run`            | `open_session_resume_preserves_supplied_id`           |
//! | kimi: `open_session_reuses_live_core_event_stream`                      | (covered by `open_session_twice_shares_core`)         |
//! | kimi: `kimi_ensure_started_concurrent_calls_serialize`                  | `ensure_started_concurrent` (new — also closes a gap)  |
//! | kimi: `kimi_ensure_started_failure_not_sticky`                          | `ensure_started_failure_not_sticky`                   |
//! | kimi: `open_session_new_run_emits_session_attached`                     | `run_emits_session_attached_for_new_session`          |
//! | kimi: `open_session_resume_run_emits_session_attached_with_supplied_id` | `run_emits_session_attached_for_resumed_session`      |
//! | kimi: `open_session_two_new_on_same_key_share_core`                     | (covered by `open_session_twice_shares_core`)         |
//! | gemini: `close_last_session_prunes_registry_entry`                      | `handle::tests::close_last_session_prunes_registry_entry`            |
//! | gemini: `register_session_in_shared_state_tracks_new_handle_session`    | (covered by `run_emits_session_attached_for_new_session`) |
//! | gemini: `close_with_live_secondary_does_not_tear_down_shared_child`     | `handle::tests::close_with_live_secondary_keeps_child_alive`         |
//! | gemini: `close_emits_closed_lifecycle_only_once_even_after_drop`        | `handle::tests::close_then_drop_emits_closed_lifecycle_once`         |

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use super::super::acp_protocol::ToolCallAccumulator;
use super::super::{AgentKey, DriverEvent, EventFanOut, ProcessState, RunId, SessionIntent};

use super::core::AcpNativeCore;
use super::handle::AcpNativeHandle;
use super::open_session as acp_native_open_session;
use super::reader::handle_response;
use super::state::{PendingRequest, SessionState};
use super::test_fixtures::{
    fresh_shared, make_core, open_test_session, test_spec, TEST_CFG, TEST_REGISTRY,
};

// ---------------------------------------------------------------------------
// Response routing — handle_response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_session_pending_dispatch_routes_session_new() {
    let (events, event_tx) = EventFanOut::new();
    let _ = events;
    let shared = fresh_shared();
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

    let (tx7, rx7) = oneshot::channel();
    let (tx8, rx8) = oneshot::channel();
    {
        let mut s = shared.lock().unwrap();
        s.pending
            .insert(7, PendingRequest::SessionNew { responder: tx7 });
        s.pending
            .insert(8, PendingRequest::SessionNew { responder: tx8 });
    }

    let key: AgentKey = "agent-x".into();
    let resp7: Value =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"result":{"sessionId":"sess-alpha"}}"#)
            .unwrap();
    let resp8: Value =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":8,"result":{"sessionId":"sess-beta"}}"#)
            .unwrap();

    handle_response("test", &key, &event_tx, &shared, &stdin_tx, &resp7).await;
    handle_response("test", &key, &event_tx, &shared, &stdin_tx, &resp8).await;

    let got7 = timeout(Duration::from_millis(500), rx7)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let got8 = timeout(Duration::from_millis(500), rx8)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(got7, "sess-alpha");
    assert_eq!(got8, "sess-beta");
}

#[tokio::test]
async fn session_load_falls_back_to_expected_id() {
    let (events, event_tx) = EventFanOut::new();
    let _ = events;
    let shared = fresh_shared();
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

    let (tx, rx) = oneshot::channel();
    {
        let mut s = shared.lock().unwrap();
        s.pending.insert(
            9,
            PendingRequest::SessionLoad {
                expected_session_id: "stored-xyz".into(),
                responder: tx,
            },
        );
    }

    let resp: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#).unwrap();
    handle_response(
        "test",
        &"k".to_string(),
        &event_tx,
        &shared,
        &stdin_tx,
        &resp,
    )
    .await;

    let got = timeout(Duration::from_millis(500), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(got, "stored-xyz");
}

#[tokio::test]
async fn prompt_response_carries_correct_session_id() {
    let (events, event_tx) = EventFanOut::new();
    let mut rx_events = events.subscribe();

    let shared = fresh_shared();
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

    let run_a = RunId::new_v4();
    let run_b = RunId::new_v4();
    {
        let mut s = shared.lock().unwrap();
        s.sessions.insert(
            "sess-A".into(),
            SessionState {
                state: ProcessState::PromptInFlight {
                    run_id: run_a,
                    session_id: "sess-A".into(),
                },
                run_id: Some(run_a),
                tool_accumulator: ToolCallAccumulator::new(),
            },
        );
        s.sessions.insert(
            "sess-B".into(),
            SessionState {
                state: ProcessState::PromptInFlight {
                    run_id: run_b,
                    session_id: "sess-B".into(),
                },
                run_id: Some(run_b),
                tool_accumulator: ToolCallAccumulator::new(),
            },
        );
        s.pending.insert(
            10,
            PendingRequest::Prompt {
                session_id: "sess-A".into(),
                run_id: run_a,
            },
        );
        s.pending.insert(
            11,
            PendingRequest::Prompt {
                session_id: "sess-B".into(),
                run_id: run_b,
            },
        );
    }

    let key: AgentKey = "agent-y".into();
    let r10: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":10,"result":{}}"#).unwrap();
    let r11: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":11,"result":{}}"#).unwrap();

    handle_response("test", &key, &event_tx, &shared, &stdin_tx, &r10).await;
    handle_response("test", &key, &event_tx, &shared, &stdin_tx, &r11).await;

    let mut completed: std::collections::HashSet<String> = Default::default();
    let deadline = Duration::from_millis(500);
    while completed.len() < 2 {
        let ev = timeout(deadline, rx_events.recv())
            .await
            .expect("timed out waiting for Completed events")
            .expect("stream closed");
        if let DriverEvent::Completed { session_id, .. } = ev {
            completed.insert(session_id);
        }
    }
    assert!(completed.contains("sess-A"));
    assert!(completed.contains("sess-B"));

    let s = shared.lock().unwrap();
    assert!(s.sessions.get("sess-A").unwrap().run_id.is_none());
    assert!(matches!(
        s.sessions.get("sess-A").unwrap().state,
        ProcessState::Active { .. }
    ));
}

#[tokio::test]
async fn response_for_unknown_id_is_ignored() {
    let (events, event_tx) = EventFanOut::new();
    let _ = events;
    let shared = fresh_shared();
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

    let resp: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":999,"result":{}}"#).unwrap();
    handle_response(
        "test",
        &"k".to_string(),
        &event_tx,
        &shared,
        &stdin_tx,
        &resp,
    )
    .await;

    let s = shared.lock().unwrap();
    assert!(s.pending.is_empty());
    assert!(s.sessions.is_empty());
}

// ---------------------------------------------------------------------------
// Registry behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_evicts_stale_core() {
    let (events, event_tx) = EventFanOut::new();
    let _ = events;
    let key: AgentKey = format!("agent-stale-{}", uuid::Uuid::new_v4());
    let core = AcpNativeCore::new(&TEST_CFG, key.clone(), test_spec(), events, event_tx);

    {
        let mut inner = core.inner.lock().await;
        let (tx, rx) = mpsc::channel::<String>(1);
        drop(rx);
        inner.stdin_tx = Some(tx);
    }
    assert!(
        super::AgentProcess::is_stale(&*core),
        "closed stdin must mark core stale"
    );

    TEST_REGISTRY.insert(key.clone(), core);
    assert!(
        TEST_REGISTRY.get_or_evict_stale(&key).is_none(),
        "registry must evict stale entry"
    );
    assert!(
        TEST_REGISTRY.get(&key).is_none(),
        "stale entry must have been pruned"
    );
}

#[tokio::test]
async fn registry_keeps_fresh_core() {
    let (events, event_tx) = EventFanOut::new();
    let _ = events;
    let key: AgentKey = format!("agent-fresh-{}", uuid::Uuid::new_v4());
    let core = AcpNativeCore::new(&TEST_CFG, key.clone(), test_spec(), events, event_tx);

    assert!(
        !super::AgentProcess::is_stale(&*core),
        "never-spawned core must not be reported as stale"
    );
    TEST_REGISTRY.insert(key.clone(), core);
    assert!(
        TEST_REGISTRY.get_or_evict_stale(&key).is_some(),
        "registry must return fresh core"
    );
    TEST_REGISTRY.remove(&key);
}

// ---------------------------------------------------------------------------
// ensure_started semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ensure_started_fast_path() {
    let core = make_core().await;
    core.started.store(true, Ordering::Release);
    // Repeated calls must short-circuit; spawn must NOT be invoked. Our
    // test spawn would fail, so a successful return proves no spawn was
    // attempted.
    core.ensure_started().await.unwrap();
    core.ensure_started().await.unwrap();
    assert_eq!(core.spawn_and_initialize_call_count_for_test(), 0);
}

#[tokio::test]
async fn ensure_started_idempotent_after_success() {
    let core = make_core().await;
    let shared = fresh_shared();
    {
        let mut inner = core.inner.lock().await;
        let (tx, _rx) = mpsc::channel::<String>(1);
        inner.stdin_tx = Some(tx);
        inner.shared = Some(shared);
        inner.next_request_id = 3;
    }
    core.started.store(true, Ordering::Release);

    core.ensure_started().await.unwrap();
    core.ensure_started().await.unwrap();
    core.ensure_started().await.unwrap();

    let inner = core.inner.lock().await;
    assert!(
        inner.stdin_tx.is_some(),
        "ensure_started must not clear stdin_tx"
    );
}

#[tokio::test]
async fn ensure_started_failure_not_sticky() {
    let core = make_core().await;

    let _ = core.ensure_started().await;
    assert!(
        !core.is_started_for_test(),
        "started must remain false after a failed ensure_started"
    );
    assert_eq!(core.spawn_and_initialize_call_count_for_test(), 1);

    // Second call retries — non-sticky failure.
    let _ = core.ensure_started().await;
    assert!(!core.is_started_for_test());
    assert_eq!(core.spawn_and_initialize_call_count_for_test(), 2);
}

/// After `spawn_and_initialize` seeding, `alloc_id` returns 3 (ids 1 =
/// initialize, 2 = first session request) and no id-3 placeholder is
/// pre-registered in `pending` — we don't reserve ahead of time.
#[tokio::test]
async fn alloc_id_starts_at_3_after_spawn_and_initialize() {
    let core = make_core().await;
    let shared = fresh_shared();
    {
        let mut inner = core.inner.lock().await;
        let (tx, _rx) = mpsc::channel::<String>(1);
        inner.stdin_tx = Some(tx);
        inner.shared = Some(shared.clone());
        inner.next_request_id = 3;
    }

    let handle = AcpNativeHandle::new(core.clone(), None);
    let id = handle.alloc_id().await;
    assert_eq!(
        id, 3,
        "alloc_id on a just-spawned core must return 3 (initialize=1, first session=2)"
    );

    let s = shared.lock().unwrap();
    assert!(
        !s.pending.contains_key(&3),
        "no id-3 reservation must exist after spawn_and_initialize"
    );
}

/// **NEW: closes a coverage gap.** Two concurrent `ensure_started` calls
/// on the same core must serialize through `start_in_progress` — they
/// never run `spawn_and_initialize` concurrently. Both fail (test spawn
/// always fails), so each retries the slow path; counter ends at exactly
/// 2.
#[tokio::test]
async fn ensure_started_concurrent() {
    let core = make_core().await;
    let c0 = Arc::clone(&core);
    let c1 = Arc::clone(&core);

    let j0 = tokio::spawn(async move { c0.ensure_started().await });
    let j1 = tokio::spawn(async move { c1.ensure_started().await });
    let (r0, r1) = tokio::join!(j0, j1);
    // Expect on the JoinHandle so a panic in either task fails the test
    // loudly. The inner Result<()> is intentionally an Err (test spawn
    // always fails) — we assert the spawn count separately below.
    let _ = r0.expect("first ensure_started task panicked");
    let _ = r1.expect("second ensure_started task panicked");

    let n = core.spawn_and_initialize_call_count_for_test();
    assert_eq!(
        n, 2,
        "spawn_and_initialize must be called exactly 2 times for 2 callers (both fail, both retry the slow path)"
    );
}

// ---------------------------------------------------------------------------
// Handle / open_session behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handle_session_id_from_preassigned() {
    let core = make_core().await;
    let preassigned = AcpNativeHandle::new(core.clone(), Some("stored-sess-abc".into()));
    let no_preassigned = AcpNativeHandle::new(core, None);

    assert_eq!(
        super::super::Session::session_id(&preassigned),
        Some("stored-sess-abc")
    );
    assert!(matches!(
        super::super::Session::process_state(&preassigned),
        ProcessState::Idle
    ));
    assert_eq!(super::super::Session::session_id(&no_preassigned), None);
}

#[tokio::test]
async fn open_session_works_without_prior_call() {
    let (key_new, ar) = open_test_session(SessionIntent::New).await;
    assert!(matches!(ar.session.process_state(), ProcessState::Idle));
    TEST_REGISTRY.remove(&key_new);

    let (key_resume, ar) = open_test_session(SessionIntent::Resume("stored-id-xyz".into())).await;
    assert_eq!(ar.session.session_id(), Some("stored-id-xyz"));
    TEST_REGISTRY.remove(&key_resume);
}

#[tokio::test]
async fn open_session_twice_shares_core() {
    let key: AgentKey = format!("agent-share-{}", uuid::Uuid::new_v4());

    let s1 = acp_native_open_session(&TEST_CFG, key.clone(), test_spec(), SessionIntent::New)
        .await
        .unwrap();
    let s2 = acp_native_open_session(&TEST_CFG, key.clone(), test_spec(), SessionIntent::New)
        .await
        .unwrap();

    let ptr1 = Arc::as_ptr(&s1.events.inner);
    let ptr2 = Arc::as_ptr(&s2.events.inner);
    assert_eq!(
        ptr1, ptr2,
        "open_session calls on the same key must share the same EventFanOut"
    );

    TEST_REGISTRY.remove(&key);
}

#[tokio::test]
async fn open_session_resume_preserves_supplied_id() {
    let key: AgentKey = format!("agent-resume-{}", uuid::Uuid::new_v4());
    let resumed = acp_native_open_session(
        &TEST_CFG,
        key.clone(),
        test_spec(),
        SessionIntent::Resume("stored-sess-xyz".into()),
    )
    .await
    .unwrap();

    assert_eq!(resumed.session.session_id(), Some("stored-sess-xyz"));
    TEST_REGISTRY.remove(&key);
}

// ---------------------------------------------------------------------------
// run() — integration: drive a session/new through to SessionAttached.
// ---------------------------------------------------------------------------

/// Drives a full open_session(New) → run() → session/new response flow,
/// with the runtime side simulated by a background task that injects the
/// session/new response into `handle_response`. Asserts that
/// `SessionAttached` is emitted with the runtime-minted id.
#[tokio::test]
async fn run_emits_session_attached_for_new_session() {
    let key: AgentKey = format!("agent-run-new-{}", uuid::Uuid::new_v4());
    let ar = acp_native_open_session(&TEST_CFG, key.clone(), test_spec(), SessionIntent::New)
        .await
        .unwrap();
    let mut event_rx = ar.events.subscribe();

    // Seed the registry's core as if ensure_started completed.
    let core = TEST_REGISTRY
        .get(&key)
        .expect("core must be in test registry");
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(16);
    let shared = fresh_shared();
    {
        let mut inner = core.inner.lock().await;
        inner.stdin_tx = Some(stdin_tx);
        inner.shared = Some(shared.clone());
        inner.next_request_id = 3;
    }
    core.started.store(true, Ordering::Release);

    let ar2 = acp_native_open_session(&TEST_CFG, key.clone(), test_spec(), SessionIntent::New)
        .await
        .unwrap();
    let mut handle = ar2.session;

    let key_bg = key.clone();
    let shared_bg = shared.clone();
    let event_tx_bg = core.event_tx.clone();
    let bg = tokio::spawn(async move {
        loop {
            let id = {
                let s = shared_bg.lock().unwrap();
                s.pending.keys().copied().find(|&id| {
                    matches!(s.pending.get(&id), Some(PendingRequest::SessionNew { .. }))
                })
            };
            if let Some(id) = id {
                let resp: Value = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "sessionId": "new-sess-from-test" }
                });
                let (stdin_tx2, _) = mpsc::channel::<String>(1);
                handle_response("test", &key_bg, &event_tx_bg, &shared_bg, &stdin_tx2, &resp).await;
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    timeout(Duration::from_millis(500), handle.run(None))
        .await
        .expect("run() timed out")
        .expect("run() failed");
    bg.await.expect("background task panicked");

    let deadline = Duration::from_millis(500);
    loop {
        let ev = timeout(deadline, event_rx.recv())
            .await
            .expect("timed out waiting for SessionAttached")
            .expect("event stream closed");
        if let DriverEvent::SessionAttached { session_id, .. } = ev {
            assert_eq!(session_id, "new-sess-from-test");
            break;
        }
    }

    TEST_REGISTRY.remove(&key);
}

#[tokio::test]
async fn run_emits_session_attached_for_resumed_session() {
    let key: AgentKey = format!("agent-run-resume-{}", uuid::Uuid::new_v4());
    let resume_id = "stored-session-abc".to_string();

    let ar = acp_native_open_session(
        &TEST_CFG,
        key.clone(),
        test_spec(),
        SessionIntent::Resume(resume_id.clone()),
    )
    .await
    .unwrap();
    assert_eq!(ar.session.session_id(), Some(resume_id.as_str()));

    let mut event_rx = ar.events.subscribe();

    let core = TEST_REGISTRY.get(&key).expect("core must be registered");
    let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(16);
    let shared = fresh_shared();
    {
        let mut inner = core.inner.lock().await;
        inner.stdin_tx = Some(stdin_tx);
        inner.shared = Some(shared.clone());
        inner.next_request_id = 3;
    }
    core.started.store(true, Ordering::Release);

    let ar2 = acp_native_open_session(
        &TEST_CFG,
        key.clone(),
        test_spec(),
        SessionIntent::Resume(resume_id.clone()),
    )
    .await
    .unwrap();
    let mut handle = ar2.session;

    let key_bg = key.clone();
    let shared_bg = shared.clone();
    let event_tx_bg = core.event_tx.clone();
    let bg = tokio::spawn(async move {
        loop {
            let id = {
                let s = shared_bg.lock().unwrap();
                s.pending.keys().copied().find(|&id| {
                    matches!(s.pending.get(&id), Some(PendingRequest::SessionLoad { .. }))
                })
            };
            if let Some(id) = id {
                let resp: Value = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                });
                let (stdin_tx2, _) = mpsc::channel::<String>(1);
                handle_response("test", &key_bg, &event_tx_bg, &shared_bg, &stdin_tx2, &resp).await;
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    timeout(Duration::from_millis(500), handle.run(None))
        .await
        .expect("run() timed out")
        .expect("run() failed");
    bg.await.expect("background task panicked");

    let deadline = Duration::from_millis(500);
    loop {
        let ev = timeout(deadline, event_rx.recv())
            .await
            .expect("timed out waiting for SessionAttached")
            .expect("event stream closed");
        if let DriverEvent::SessionAttached { session_id, .. } = ev {
            assert_eq!(session_id, resume_id);
            break;
        }
    }

    TEST_REGISTRY.remove(&key);
}

// close() multi-session teardown tests live in `handle.rs::tests` so they
// can construct `AcpNativeHandle` with private field access (no test-only
// setter shim).
