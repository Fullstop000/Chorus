//! Verifies AgentManager::process_state returns None when no managed
//! agent exists. Additional variants (Active/Failed/etc.) are covered
//! by Task 3.0's eviction test and later integration tests.

use chorus::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn process_state_none_when_no_agent_managed() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("db.sqlite");
    let store = Arc::new(
        Store::open(db_path.to_str().expect("utf-8 path")).unwrap(),
    );
    let mgr = chorus::agent::manager::AgentManager::new_for_test(
        store,
        dir.path().to_path_buf(),
    );
    let state: Option<chorus::agent::drivers::ProcessState> =
        mgr.process_state("nonexistent").await;
    assert!(state.is_none());
}
