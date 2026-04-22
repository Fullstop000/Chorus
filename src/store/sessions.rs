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
    pub fn record_session(
        &self,
        agent_id: &str,
        session_id: &str,
        runtime: &str,
    ) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_store() -> (Store, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("db.sqlite");
        let store = Store::open(db_path.to_str().expect("utf-8 path")).unwrap();
        store.conn.lock().unwrap().execute(
            "INSERT INTO agents (id, name, display_name, runtime, model) VALUES ('a1', 'a', 'A', 'fake', 'fake')",
            [],
        ).unwrap();
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
    fn migration_copies_existing_session_id() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(include_str!("schema.sql")).unwrap();
            // Re-add the legacy column to simulate an upgrade path.
            let _ = conn.execute("ALTER TABLE agents ADD COLUMN session_id TEXT", []);
            conn.execute(
                "INSERT INTO agents (id, name, display_name, runtime, model, session_id) VALUES ('a1', 'a', 'A', 'fake', 'fake', 'legacy-sess')",
                [],
            ).unwrap();
        }
        let store = Store::open(db_path.to_str().expect("utf-8 path")).unwrap();
        let got = store.get_active_session("a1").unwrap().unwrap();
        assert_eq!(got.session_id, "legacy-sess");
    }
}
