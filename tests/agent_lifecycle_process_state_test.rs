//! AgentLifecycle::process_state returns None for unmanaged agents on the
//! production AgentManager impl.

use chorus::agent::lifecycle::AgentLifecycle;
use chorus::agent::manager::AgentManager;
use chorus::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn process_state_none_for_unmanaged_in_production_impl() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("db.sqlite");
    let store = Arc::new(
        Store::open(db_path.to_str().expect("utf-8 path")).unwrap(),
    );
    let mgr: Arc<dyn AgentLifecycle> = Arc::new(
        AgentManager::new_for_test(store, dir.path().to_path_buf()),
    );
    assert!(mgr.process_state("nonexistent").await.is_none());
}
