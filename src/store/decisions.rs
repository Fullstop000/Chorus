//! `decisions` table CRUD.
//!
//! Backs the decision-inbox subsystem (`crate::decision`). v1 is the
//! minimum mechanism: insert, list-open, get, CAS-resolve. No backoff,
//! no per-session queue, no `delivery_failed` terminal state — those
//! ship in v2 once the loop runs once.
//!
//! See `docs/DECISIONS.md` for the lifecycle and r7 design.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;
use uuid::Uuid;

use super::{parse_datetime, Store};
use crate::decision::{DecisionPayload, ResolvePayload};

/// One row from `decisions`. `payload_json` keeps the agent-authored
/// shape (headline, question, options, recommended_key, context) as a
/// serialized blob; query callers deserialize via `serde_json::from_str`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub status: DecisionStatus,
    pub payload_json: String,
    pub picked_key: Option<String>,
    pub picked_note: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionStatus {
    Open,
    Resolved,
}

impl DecisionStatus {
    fn from_db(s: &str) -> Self {
        match s {
            "resolved" => DecisionStatus::Resolved,
            // Anything else is treated as Open. The schema only allows
            // 'open' | 'resolved' so this is just defensive.
            _ => DecisionStatus::Open,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Open => "open",
            DecisionStatus::Resolved => "resolved",
        }
    }
}

impl DecisionRow {
    /// Deserialize the stored payload. Returns the parsed shape so
    /// callers can read `headline`, `options`, etc. without having to
    /// reach into the JSON string themselves.
    pub fn payload(&self) -> Result<DecisionPayload> {
        Ok(serde_json::from_str(&self.payload_json)?)
    }
}

impl Store {
    /// Insert a new decision row. Returns the freshly minted decision id
    /// (a UUID v4 string). The `payload` is serialized verbatim into
    /// `payload_json`. Identity columns are filled by the caller from
    /// the bridge auth context plus the agent's active-run channel.
    pub fn create_decision(
        &self,
        workspace_id: &str,
        channel_id: &str,
        agent_id: &str,
        session_id: &str,
        payload: &DecisionPayload,
    ) -> Result<DecisionRow> {
        let id = Uuid::new_v4().to_string();
        let payload_json = serde_json::to_string(payload)?;
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO decisions (
                id, workspace_id, channel_id, agent_id, session_id,
                status, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
            params![
                id,
                workspace_id,
                channel_id,
                agent_id,
                session_id,
                payload_json
            ],
        )?;
        // Read back the row so the caller gets the canonical
        // server-assigned `created_at` instead of guessing locally.
        drop(conn);
        self.get_decision(&id)?
            .ok_or_else(|| anyhow::anyhow!("decision row vanished immediately after insert"))
    }

    /// Fetch one decision by id, or `None` if it doesn't exist.
    pub fn get_decision(&self, id: &str) -> Result<Option<DecisionRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, channel_id, agent_id, session_id,
                    created_at, status, payload_json,
                    picked_key, picked_note, resolved_at
             FROM decisions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], decision_row_from_columns)?;
        Ok(rows.next().transpose()?)
    }

    /// List decisions in a workspace, optionally filtered by status.
    /// Newest first. Workspaces and channels delete-cascade via the
    /// foreign-key constraint, so callers don't have to defensively
    /// filter for orphans.
    pub fn list_decisions(
        &self,
        workspace_id: &str,
        status: Option<DecisionStatus>,
    ) -> Result<Vec<DecisionRow>> {
        let conn = self.lock_conn();
        let mut stmt;
        let rows = if let Some(s) = status {
            stmt = conn.prepare(
                "SELECT id, workspace_id, channel_id, agent_id, session_id,
                        created_at, status, payload_json,
                        picked_key, picked_note, resolved_at
                 FROM decisions
                 WHERE workspace_id = ?1 AND status = ?2
                 ORDER BY created_at DESC",
            )?;
            stmt.query_map(params![workspace_id, s.as_str()], decision_row_from_columns)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt = conn.prepare(
                "SELECT id, workspace_id, channel_id, agent_id, session_id,
                        created_at, status, payload_json,
                        picked_key, picked_note, resolved_at
                 FROM decisions
                 WHERE workspace_id = ?1
                 ORDER BY created_at DESC",
            )?;
            stmt.query_map(params![workspace_id], decision_row_from_columns)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    /// Revert a Resolved row back to Open, clearing pick state. Used by
    /// the resolve handler when `resume_with_prompt` fails after the
    /// CAS has already flipped the status — the human gets to re-pick
    /// once whatever broke (driver, runtime, env) is fixed.
    ///
    /// Idempotent: returns Ok even if the row is already Open or the
    /// id doesn't exist. Caller has logged the original failure;
    /// surfacing a second error here would only obscure the root cause.
    pub fn revert_decision_to_open(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE decisions
             SET status = 'open',
                 picked_key = NULL,
                 picked_note = NULL,
                 resolved_at = NULL
             WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// CAS resolve. Atomically transitions a row from Open to Resolved
    /// and stamps the picked_key/note/resolved_at columns. Returns:
    /// - `Ok(Some(row))` on success — the row is now Resolved.
    /// - `Ok(None)` if no row matched (already resolved, or unknown id).
    ///   The caller should return 409 to the human — first-pick wins;
    ///   second pick races and loses.
    /// - `Err(_)` on a database error.
    ///
    /// The picked_key match against existing options is the caller's
    /// job: handlers parse the payload and validate the picked_key
    /// before invoking this method.
    pub fn resolve_decision_cas(
        &self,
        id: &str,
        resolve: &ResolvePayload,
    ) -> Result<Option<DecisionRow>> {
        let conn = self.lock_conn();
        let updated = conn.execute(
            "UPDATE decisions
             SET status = 'resolved',
                 picked_key = ?2,
                 picked_note = ?3,
                 resolved_at = datetime('now')
             WHERE id = ?1 AND status = 'open'",
            params![id, resolve.picked_key, resolve.note],
        )?;
        drop(conn);
        if updated == 0 {
            return Ok(None);
        }
        self.get_decision(id)
    }
}

fn decision_row_from_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRow> {
    let created_at: String = row.get(5)?;
    let status: String = row.get(6)?;
    let resolved_at: Option<String> = row.get(10)?;
    Ok(DecisionRow {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        channel_id: row.get(2)?,
        agent_id: row.get(3)?,
        session_id: row.get(4)?,
        created_at: parse_datetime(&created_at),
        status: DecisionStatus::from_db(&status),
        payload_json: row.get(7)?,
        picked_key: row.get(8)?,
        picked_note: row.get(9)?,
        resolved_at: resolved_at.map(|s| parse_datetime(&s)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::OptionPayload;
    use tempfile::TempDir;

    fn fresh_store() -> (Store, TempDir, String, String) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("db.sqlite");
        let store = Store::open(db_path.to_str().unwrap()).unwrap();
        let alice = store.ensure_human_with_id("alice", "alice").unwrap();
        let workspace = store.create_local_workspace("Test", &alice.id).unwrap();
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO channels (id, workspace_id, name, channel_type) VALUES ('c1', ?1, 'general', 'channel')",
            params![workspace.id],
        ).unwrap();
        conn.execute(
            "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
             VALUES ('a1', ?1, 'a', 'A', 'fake', 'fake')",
            params![workspace.id],
        )
        .unwrap();
        drop(conn);
        (store, dir, workspace.id, "c1".to_string())
    }

    fn sample_payload() -> DecisionPayload {
        DecisionPayload {
            headline: "PR #121: archived-channel fix".into(),
            question: "How do you want to land this?".into(),
            options: vec![
                OptionPayload {
                    key: "a".into(),
                    label: "Merge as-is".into(),
                    body: "Squash and merge.".into(),
                },
                OptionPayload {
                    key: "b".into(),
                    label: "Request changes".into(),
                    body: "Add the e2e test first.".into(),
                },
            ],
            recommended_key: "a".into(),
            context: String::new(),
        }
    }

    #[test]
    fn create_then_get_round_trips() {
        let (store, _dir, workspace_id, channel_id) = fresh_store();
        let payload = sample_payload();
        let row = store
            .create_decision(&workspace_id, &channel_id, "a1", "sess-1", &payload)
            .expect("create");
        assert_eq!(row.status, DecisionStatus::Open);
        assert_eq!(row.agent_id, "a1");
        assert_eq!(row.session_id, "sess-1");
        let fetched = store.get_decision(&row.id).unwrap().expect("fetch");
        assert_eq!(fetched, row);
        assert_eq!(fetched.payload().unwrap(), payload);
    }

    #[test]
    fn list_filters_by_status() {
        let (store, _dir, workspace_id, channel_id) = fresh_store();
        let r1 = store
            .create_decision(&workspace_id, &channel_id, "a1", "s", &sample_payload())
            .unwrap();
        let r2 = store
            .create_decision(&workspace_id, &channel_id, "a1", "s", &sample_payload())
            .unwrap();

        // resolve r1 — list-open should now only return r2.
        let resolved = store
            .resolve_decision_cas(
                &r1.id,
                &ResolvePayload {
                    picked_key: "a".into(),
                    note: None,
                },
            )
            .unwrap()
            .expect("CAS should succeed");
        assert_eq!(resolved.status, DecisionStatus::Resolved);
        assert_eq!(resolved.picked_key.as_deref(), Some("a"));

        let open = store
            .list_decisions(&workspace_id, Some(DecisionStatus::Open))
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, r2.id);

        let resolved_list = store
            .list_decisions(&workspace_id, Some(DecisionStatus::Resolved))
            .unwrap();
        assert_eq!(resolved_list.len(), 1);
        assert_eq!(resolved_list[0].id, r1.id);
    }

    #[test]
    fn cas_loses_on_double_resolve() {
        let (store, _dir, workspace_id, channel_id) = fresh_store();
        let row = store
            .create_decision(&workspace_id, &channel_id, "a1", "s", &sample_payload())
            .unwrap();
        let resolve = ResolvePayload {
            picked_key: "a".into(),
            note: Some("first".into()),
        };
        let first = store.resolve_decision_cas(&row.id, &resolve).unwrap();
        assert!(first.is_some(), "first CAS should win");

        let second = store
            .resolve_decision_cas(
                &row.id,
                &ResolvePayload {
                    picked_key: "b".into(),
                    note: Some("second".into()),
                },
            )
            .unwrap();
        assert!(second.is_none(), "second CAS races and loses (=> 409)");

        // Row state must reflect the FIRST pick, not the second.
        let after = store.get_decision(&row.id).unwrap().unwrap();
        assert_eq!(after.picked_key.as_deref(), Some("a"));
        assert_eq!(after.picked_note.as_deref(), Some("first"));
    }

    #[test]
    fn cas_returns_none_for_unknown_id() {
        let (store, _dir, _ws, _ch) = fresh_store();
        let none = store
            .resolve_decision_cas(
                "nonexistent",
                &ResolvePayload {
                    picked_key: "a".into(),
                    note: None,
                },
            )
            .unwrap();
        assert!(none.is_none());
    }
}
