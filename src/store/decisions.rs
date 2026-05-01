//! `decisions` table accessors.
//!
//! Decisions emitted by agents via `chorus_create_decision`. Lifecycle:
//! agent → `create_decision` (status=open) → human picks in inbox →
//! `resolve_decision_cas` (status=resolved) → `revert_decision_to_open` if
//! the resume_with_prompt envelope delivery fails.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Open,
    Resolved,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Open => "open",
            DecisionStatus::Resolved => "resolved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(DecisionStatus::Open),
            "resolved" => Some(DecisionStatus::Resolved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub created_at: String,
    pub status: DecisionStatus,
    pub payload_json: String,
    pub picked_key: Option<String>,
    pub picked_note: Option<String>,
    pub resolved_at: Option<String>,
}

impl Store {
    /// Insert a freshly-emitted decision (status=open).
    pub fn create_decision(
        &self,
        id: &str,
        workspace_id: &str,
        channel_id: &str,
        agent_id: &str,
        session_id: &str,
        payload_json: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO decisions
                (id, workspace_id, channel_id, agent_id, session_id, status, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
            params![
                id,
                workspace_id,
                channel_id,
                agent_id,
                session_id,
                payload_json
            ],
        )?;
        Ok(())
    }

    pub fn get_decision(&self, id: &str) -> Result<Option<DecisionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, channel_id, agent_id, session_id,
                    created_at, status, payload_json, picked_key, picked_note, resolved_at
             FROM decisions WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], row_to_decision).optional()?;
        Ok(row)
    }

    /// List decisions for a workspace, optionally filtered by status.
    /// Returns most recent first.
    pub fn list_decisions(
        &self,
        workspace_id: &str,
        status: Option<DecisionStatus>,
    ) -> Result<Vec<DecisionRow>> {
        let conn = self.conn.lock().unwrap();
        let rows: Vec<DecisionRow> = if let Some(s) = status {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, channel_id, agent_id, session_id,
                        created_at, status, payload_json, picked_key, picked_note, resolved_at
                 FROM decisions WHERE workspace_id = ?1 AND status = ?2
                 ORDER BY created_at DESC",
            )?;
            let collected = stmt
                .query_map(params![workspace_id, s.as_str()], row_to_decision)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, channel_id, agent_id, session_id,
                        created_at, status, payload_json, picked_key, picked_note, resolved_at
                 FROM decisions WHERE workspace_id = ?1
                 ORDER BY created_at DESC",
            )?;
            let collected = stmt
                .query_map(params![workspace_id], row_to_decision)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        };
        Ok(rows)
    }

    /// CAS-protected resolve. Updates row only if it's still `open`.
    /// Returns `true` on success, `false` if the row was already resolved
    /// or doesn't exist. Caller checks the bool to detect the race.
    pub fn resolve_decision_cas(
        &self,
        id: &str,
        picked_key: &str,
        picked_note: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE decisions
             SET status = 'resolved',
                 picked_key = ?2,
                 picked_note = ?3,
                 resolved_at = datetime('now')
             WHERE id = ?1 AND status = 'open'",
            params![id, picked_key, picked_note],
        )?;
        Ok(n == 1)
    }

    /// Roll a resolved decision back to open. Used when `resume_with_prompt`
    /// fails to deliver the envelope — the human's pick should not be lost
    /// silently, so we re-arm the decision and surface the error.
    pub fn revert_decision_to_open(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE decisions
             SET status = 'open',
                 picked_key = NULL,
                 picked_note = NULL,
                 resolved_at = NULL
             WHERE id = ?1 AND status = 'resolved'",
            params![id],
        )?;
        Ok(())
    }
}

fn row_to_decision(row: &rusqlite::Row) -> rusqlite::Result<DecisionRow> {
    let status_str: String = row.get(6)?;
    let status = DecisionStatus::parse(&status_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            format!("invalid decision status: {status_str}").into(),
        )
    })?;
    Ok(DecisionRow {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        channel_id: row.get(2)?,
        agent_id: row.get(3)?,
        session_id: row.get(4)?,
        created_at: row.get(5)?,
        status,
        payload_json: row.get(7)?,
        picked_key: row.get(8)?,
        picked_note: row.get(9)?,
        resolved_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, Store) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("chorus.db");
        let store = Store::open(db_path.to_str().unwrap()).unwrap();
        (tmp, store)
    }

    fn seed_agent_and_workspace(store: &Store) -> (String, String) {
        // Minimal seed so the FK on agent_id is satisfied.
        let conn = store.conn.lock().unwrap();
        let workspace_id = "ws-1".to_string();
        conn.execute(
            "INSERT INTO workspaces (id, name, slug) VALUES (?1, 'test', 'test')",
            params![workspace_id],
        )
        .unwrap();
        let agent_id = "agent-1".to_string();
        conn.execute(
            "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
             VALUES (?1, ?2, 'bot', 'Bot', 'claude', 'sonnet')",
            params![agent_id, workspace_id],
        )
        .unwrap();
        (workspace_id, agent_id)
    }

    #[test]
    fn create_and_get_round_trip() {
        let (_tmp, store) = fresh_store();
        let (ws, ag) = seed_agent_and_workspace(&store);
        store
            .create_decision("d1", &ws, "ch1", &ag, "sess1", r#"{"k":"v"}"#)
            .unwrap();
        let row = store.get_decision("d1").unwrap().unwrap();
        assert_eq!(row.id, "d1");
        assert_eq!(row.status, DecisionStatus::Open);
        assert_eq!(row.payload_json, r#"{"k":"v"}"#);
        assert!(row.picked_key.is_none());
    }

    #[test]
    fn list_filters_by_status() {
        let (_tmp, store) = fresh_store();
        let (ws, ag) = seed_agent_and_workspace(&store);
        store
            .create_decision("a", &ws, "c", &ag, "s", "{}")
            .unwrap();
        store
            .create_decision("b", &ws, "c", &ag, "s", "{}")
            .unwrap();
        assert!(store.resolve_decision_cas("a", "X", None).unwrap());
        let open = store
            .list_decisions(&ws, Some(DecisionStatus::Open))
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, "b");
        let resolved = store
            .list_decisions(&ws, Some(DecisionStatus::Resolved))
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "a");
        let all = store.list_decisions(&ws, None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn cas_returns_false_on_double_resolve() {
        let (_tmp, store) = fresh_store();
        let (ws, ag) = seed_agent_and_workspace(&store);
        store
            .create_decision("d1", &ws, "c", &ag, "s", "{}")
            .unwrap();
        assert!(store.resolve_decision_cas("d1", "A", None).unwrap());
        // Second pick must fail without erroring.
        assert!(!store.resolve_decision_cas("d1", "B", None).unwrap());
        let row = store.get_decision("d1").unwrap().unwrap();
        assert_eq!(row.picked_key.as_deref(), Some("A"));
    }

    #[test]
    fn revert_reopens_resolved_row() {
        let (_tmp, store) = fresh_store();
        let (ws, ag) = seed_agent_and_workspace(&store);
        store
            .create_decision("d1", &ws, "c", &ag, "s", "{}")
            .unwrap();
        store.resolve_decision_cas("d1", "A", Some("note")).unwrap();
        store.revert_decision_to_open("d1").unwrap();
        let row = store.get_decision("d1").unwrap().unwrap();
        assert_eq!(row.status, DecisionStatus::Open);
        assert!(row.picked_key.is_none());
        assert!(row.picked_note.is_none());
        assert!(row.resolved_at.is_none());
        // CAS must work again after revert.
        assert!(store.resolve_decision_cas("d1", "B", None).unwrap());
    }
}
