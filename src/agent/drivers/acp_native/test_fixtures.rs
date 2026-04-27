//! Shared test fixtures for `acp_native::tests` and the inline test
//! modules in `core.rs`, `handle.rs`, `reader.rs`. Centralized so each
//! test site doesn't redefine the same `TestConfig` / `test_spec` /
//! `fresh_shared` helpers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::super::SessionAttachment;
use super::super::SessionIntent;
use super::super::{AgentKey, AgentRegistry, AgentSpec, EventFanOut};
use super::core::AcpNativeCore;
use super::state::SharedReaderState;
use super::{
    open_session as acp_native_open_session, AcpDriverConfig, InitPromptStrategy, SpawnFut,
};

use crate::agent::AgentRuntime;

pub(super) fn test_spec() -> AgentSpec {
    AgentSpec {
        display_name: "test".into(),
        description: None,
        system_prompt: None,
        model: "test-model".into(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: PathBuf::from("/tmp"),
        bridge_endpoint: "http://127.0.0.1:1".into(),
    }
}

pub(super) fn test_mcp_servers(_endpoint: &str, _key: &str) -> Value {
    serde_json::json!([])
}

/// Spawn function that always fails. Lets tests drive `ensure_started`
/// without a real runtime binary while observing the
/// failure-non-stickiness invariant.
pub(super) fn test_spawn_always_fails(_spec: Arc<AgentSpec>, _key: AgentKey) -> SpawnFut {
    Box::pin(async move { Err(anyhow::anyhow!("test spawn always fails")) })
}

pub(super) static TEST_REGISTRY: AgentRegistry<AcpNativeCore> = AgentRegistry::new();

pub(super) static TEST_CFG: AcpDriverConfig = AcpDriverConfig {
    name: "test",
    runtime: AgentRuntime::Kimi, // Borrowed variant; runtime tag isn't asserted in shared tests.
    init_prompt_strategy: InitPromptStrategy::Immediate,
    initialized_notification_payload: None,
    session_load_includes_mcp: true,
    emit_starting_lifecycle: false,
    build_session_new_mcp_servers: test_mcp_servers,
    build_first_prompt_prefix: None,
    spawn_child: test_spawn_always_fails,
    registry: &TEST_REGISTRY,
};

pub(super) fn fresh_shared() -> Arc<Mutex<SharedReaderState>> {
    Arc::new(Mutex::new(SharedReaderState {
        phase: super::super::acp_protocol::AcpPhase::Active,
        sessions: HashMap::new(),
        pending: HashMap::new(),
        closed_emitted: Arc::new(AtomicBool::new(false)),
        initialized_notification: None,
    }))
}

pub(super) async fn make_core() -> Arc<AcpNativeCore> {
    let (events, event_tx) = EventFanOut::new();
    let key: AgentKey = format!("test-{}", uuid::Uuid::new_v4());
    AcpNativeCore::new(&TEST_CFG, key, test_spec(), events, event_tx)
}

pub(super) async fn open_test_session(intent: SessionIntent) -> (AgentKey, SessionAttachment) {
    let key: AgentKey = format!("test-open-{}", uuid::Uuid::new_v4());
    let res = acp_native_open_session(&TEST_CFG, key.clone(), test_spec(), intent)
        .await
        .expect("open_session must succeed");
    (key, res)
}
