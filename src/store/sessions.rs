//! `agent_sessions` accessors.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;

use super::{parse_datetime, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSession {
    pub session_id: String,
    pub runtime: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

impl Store {
    pub fn get_active_session(&self, agent_id: &str) -> Result<Option<AgentSession>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, runtime, created_at, last_used_at
             FROM agent_sessions
             WHERE agent_id = ?1 AND is_active = 1
             ORDER BY last_used_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![agent_id], |row| {
            let created_at: String = row.get(2)?;
            let last_used_at: String = row.get(3)?;
            Ok(AgentSession {
                session_id: row.get(0)?,
                runtime: row.get(1)?,
                created_at: parse_datetime(&created_at),
                last_used_at: parse_datetime(&last_used_at),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Upsert: marks all other sessions for this agent inactive and
    /// inserts (or refreshes) the named one as active. Atomic — if the
    /// insert/upsert fails, the deactivation is rolled back so the agent
    /// is never left with zero active sessions.
    pub fn record_session(&self, agent_id: &str, session_id: &str, runtime: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE agent_sessions SET is_active = 0 WHERE agent_id = ?1",
            params![agent_id],
        )?;
        tx.execute(
            "INSERT INTO agent_sessions (agent_id, session_id, runtime, is_active, created_at, last_used_at)
             VALUES (?1, ?2, ?3, 1, datetime('now'), datetime('now'))
             ON CONFLICT(agent_id, session_id) DO UPDATE SET
               runtime = excluded.runtime,
               is_active = 1,
               last_used_at = datetime('now')",
            params![agent_id, session_id, runtime],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_active_session(&self, agent_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_sessions SET is_active = 0 WHERE agent_id = ?1",
            params![agent_id],
        )?;
        Ok(())
    }

    /// Drop every `agent_sessions` row for the given agent. Called from
    /// the bridge's reconcile when an agent leaves the desired set —
    /// the bridge runs with FK enforcement off so cascade does not fire
    /// automatically.
    pub fn delete_sessions_for_agent(&self, agent_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_sessions WHERE agent_id = ?1",
            params![agent_id],
        )?;
        Ok(())
    }

    /// Bulk cleanup: drop every `agent_sessions` row whose `agent_id` is
    /// NOT in the provided keep-list. Called once per `bridge.target`
    /// reconcile — handles three cases that the per-stop-event path
    /// alone misses:
    ///   1. Bridge restarts; an agent was removed from desired while
    ///      offline. The stop loop never fires for it.
    ///   2. Two reconciles arrive in quick succession with different
    ///      sets — anything dropped between them is reaped here.
    ///   3. First-ever connect with stale rows from a prior incarnation.
    ///
    /// An empty `keep` slice wipes the entire table (no agent is
    /// desired right now).
    pub fn delete_sessions_for_agents_not_in(&self, keep: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if keep.is_empty() {
            conn.execute("DELETE FROM agent_sessions", [])?;
            return Ok(());
        }
        let placeholders = std::iter::repeat_n("?", keep.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM agent_sessions WHERE agent_id NOT IN ({placeholders})");
        conn.execute(&sql, rusqlite::params_from_iter(keep.iter()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_store() -> (Store, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("db.sqlite");
        let store = Store::open(db_path.to_str().expect("utf-8 path")).unwrap();
        let alice = store.ensure_human_with_id("alice", "alice").unwrap();
        let (workspace, _event) = store.create_local_workspace("Test", &alice.id).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model, machine_id)
             VALUES ('a1', ?1, 'a', 'A', 'fake', 'fake', 'test-machine')",
                params![workspace.id],
            )
            .unwrap();
        (store, dir)
    }

    #[test]
    fn empty_returns_none() {
        let (store, _dir) = fresh_store();
        assert!(store.get_active_session("a1").unwrap().is_none());
    }

    #[test]
    fn record_then_get_returns_session() {
        let (store, _dir) = fresh_store();
        store.record_session("a1", "sess-1", "fake").unwrap();
        let got = store.get_active_session("a1").unwrap().unwrap();
        assert_eq!(got.session_id, "sess-1");
    }

    #[test]
    fn record_supersedes_previous_active() {
        let (store, _dir) = fresh_store();
        store.record_session("a1", "sess-1", "fake").unwrap();
        store.record_session("a1", "sess-2", "fake").unwrap();
        let got = store.get_active_session("a1").unwrap().unwrap();
        assert_eq!(got.session_id, "sess-2");
    }

    #[test]
    fn clear_makes_get_return_none() {
        let (store, _dir) = fresh_store();
        store.record_session("a1", "sess-1", "fake").unwrap();
        store.clear_active_session("a1").unwrap();
        assert!(store.get_active_session("a1").unwrap().is_none());
    }

    #[test]
    fn delete_sessions_for_agent_removes_all_rows() {
        let (store, _dir) = fresh_store();
        store.record_session("a1", "sess-1", "fake").unwrap();
        store.record_session("a1", "sess-2", "fake").unwrap();
        store.delete_sessions_for_agent("a1").unwrap();
        assert!(store.get_active_session("a1").unwrap().is_none());
        // record_session should now insert a fresh row, not collide.
        store.record_session("a1", "sess-3", "fake").unwrap();
        assert_eq!(
            store.get_active_session("a1").unwrap().unwrap().session_id,
            "sess-3"
        );
    }

    #[test]
    fn delete_sessions_for_agents_not_in_drops_unkept() {
        // Bridge use: bulk cleanup at reconcile start. Two agents have
        // sessions; the keep-list mentions only one — the other's rows
        // are wiped, the kept one survives.
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("bridge.db");
        let store = Store::open_for_bridge(db_path.to_str().unwrap()).unwrap();
        store
            .record_session("keep-id", "sess-keep", "fake")
            .unwrap();
        store
            .record_session("drop-id", "sess-drop", "fake")
            .unwrap();
        store
            .delete_sessions_for_agents_not_in(&["keep-id".to_string()])
            .unwrap();
        assert!(store.get_active_session("keep-id").unwrap().is_some());
        assert!(store.get_active_session("drop-id").unwrap().is_none());
    }

    #[test]
    fn delete_sessions_for_agents_not_in_with_empty_keep_wipes_all() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("bridge.db");
        let store = Store::open_for_bridge(db_path.to_str().unwrap()).unwrap();
        store.record_session("a", "sess-a", "fake").unwrap();
        store.record_session("b", "sess-b", "fake").unwrap();
        store.delete_sessions_for_agents_not_in(&[]).unwrap();
        assert!(store.get_active_session("a").unwrap().is_none());
        assert!(store.get_active_session("b").unwrap().is_none());
    }

    /// The bridge opens its store with `foreign_keys=OFF` so it can
    /// write resume cursors without a corresponding `agents` row. Pin
    /// that contract so a future regression flipping the PRAGMA back
    /// gets caught here.
    #[test]
    fn bridge_store_writes_session_without_agent_row() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("bridge.db");
        let store = Store::open_for_bridge(db_path.to_str().unwrap()).unwrap();
        // No agents row inserted — the bridge's `agents` table stays empty.
        store
            .record_session("orphan-id", "sess-bridge", "claude")
            .expect("FK off allows session insert without agent row");
        let got = store.get_active_session("orphan-id").unwrap().unwrap();
        assert_eq!(got.session_id, "sess-bridge");
        store.delete_sessions_for_agent("orphan-id").unwrap();
        assert!(store.get_active_session("orphan-id").unwrap().is_none());
    }
}
