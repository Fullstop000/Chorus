use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::events::NewEvent;
use super::{parse_datetime, Store};

/// A named group of agents (and optional human observers) that collaborate on tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    /// UUID primary key.
    pub id: String,
    /// URL-safe slug (matches backing channel name when present).
    pub name: String,
    /// Human-facing team title.
    pub display_name: String,
    /// Backing channel id for the team's shared room, if it exists.
    pub channel_id: Option<String>,
    /// Collaboration strategy: "swarm" (all agents decide together) or "leader_operators".
    pub collaboration_model: String,
    /// For leader_operators model, the agent designated as the leader.
    pub leader_agent_name: Option<String>,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
}

/// A single member (agent or human) within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Foreign key to `teams.id`.
    pub team_id: String,
    /// Handle (agent name or human username).
    pub member_name: String,
    /// `agent` or `human` string from DB.
    pub member_type: String,
    /// Agent UUID or human username — always populated at insert.
    pub member_id: String,
    /// Role within the team (e.g. leader, operator, observer).
    pub role: String,
    /// When the member joined the team.
    pub joined_at: DateTime<Utc>,
}

/// Summary of a team membership for use in agent system prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMembership {
    /// Team slug.
    pub team_name: String,
    /// Member's role string in that team.
    pub role: String,
}

impl Team {
    /// Parse the standard 7-column team listing row: id, name, display_name, channel_id (join), collaboration_model, leader_agent_name, created_at.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            display_name: row.get(2)?,
            channel_id: row.get(3)?,
            collaboration_model: row.get(4)?,
            leader_agent_name: row.get(5)?,
            created_at: super::parse_datetime(&row.get::<_, String>(6)?),
        })
    }
}

impl Store {
    fn append_team_coordination_event_tx(
        tx: &Transaction<'_>,
        team_id: &str,
        event_type: &'static str,
        actor_name: Option<&str>,
        actor_type: Option<&str>,
        caused_by_kind: Option<&'static str>,
        payload: serde_json::Value,
    ) -> Result<i64> {
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type,
                scope_kind: "team",
                scope_id: format!("team:{team_id}"),
                channel_id: None,
                channel_name: None,
                thread_parent_id: None,
                actor_name,
                actor_type,
                caused_by_kind,
                payload,
            },
        )
    }

    pub fn record_team_delegation_requested(
        &self,
        team_id: &str,
        trigger_message_id: &str,
        source_channel_name: &str,
        actor_name: &str,
        actor_type: &str,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let last_event_id = Self::append_team_coordination_event_tx(
            &tx,
            team_id,
            "team.delegation_requested",
            Some(actor_name),
            Some(actor_type),
            Some("forward_team_mentions"),
            json!({
                "triggerMessageId": trigger_message_id,
                "sourceChannelName": source_channel_name,
            }),
        )?;
        tx.commit()?;
        let _ = self.event_tx.send(last_event_id);
        Ok(())
    }

    pub fn record_team_deliberation_requested(
        &self,
        team_id: &str,
        trigger_message_id: &str,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let last_event_id = Self::append_team_coordination_event_tx(
            &tx,
            team_id,
            "team.deliberation_requested",
            Some("system"),
            None,
            Some("forward_team_mentions"),
            json!({
                "triggerMessageId": trigger_message_id,
            }),
        )?;
        tx.commit()?;
        let _ = self.event_tx.send(last_event_id);
        Ok(())
    }

    /// Create a new team and return its generated UUID.
    pub fn create_team(
        &self,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO teams (id, name, display_name, collaboration_model, leader_agent_name)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                name,
                display_name,
                collaboration_model,
                leader_agent_name
            ],
        )?;
        Ok(id)
    }

    /// Look up a team by its unique short name. Returns `None` if not found.
    pub fn get_team(&self, name: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.name, t.display_name, c.id, t.collaboration_model, t.leader_agent_name, t.created_at
             FROM teams t
             LEFT JOIN channels c ON c.name = t.name AND c.channel_type = 'team' AND c.archived = 0
             WHERE t.name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], Team::from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Look up a team by its UUID. Returns `None` if not found.
    pub fn get_team_by_id(&self, id: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.name, t.display_name, c.id, t.collaboration_model, t.leader_agent_name, t.created_at
             FROM teams t
             LEFT JOIN channels c ON c.name = t.name AND c.channel_type = 'team' AND c.archived = 0
             WHERE t.id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Team::from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// List all teams ordered by name.
    pub fn get_teams(&self) -> Result<Vec<Team>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT t.id, t.name, t.display_name, c.id, t.collaboration_model, t.leader_agent_name, t.created_at
                 FROM teams t
                 LEFT JOIN channels c ON c.name = t.name AND c.channel_type = 'team' AND c.archived = 0
                 ORDER BY t.name",
            )?
            .query_map([], Team::from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Update mutable fields of an existing team. Returns an error if the id is not found.
    pub fn update_team(
        &self,
        id: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE teams SET display_name = ?1, collaboration_model = ?2, leader_agent_name = ?3
             WHERE id = ?4",
            params![display_name, collaboration_model, leader_agent_name, id],
        )?;
        if n == 0 {
            return Err(anyhow!("team not found: {}", id));
        }
        Ok(())
    }

    /// Delete a team by id. Cascades to team_members, team_task_quorum, and team_task_signals
    /// because the schema declares ON DELETE CASCADE and PRAGMA foreign_keys=ON is set at open.
    pub fn delete_team(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM teams WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Add a member to a team. Silently no-ops if the (team_id, member_name) pair already exists.
    pub fn create_team_member(
        &self,
        team_id: &str,
        member_name: &str,
        member_type: &str,
        member_id: &str,
        role: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO team_members (team_id, member_name, member_type, member_id, role)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![team_id, member_name, member_type, member_id, role],
        )?;
        Ok(())
    }

    /// Remove a single member from a team.
    pub fn delete_team_member(&self, team_id: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM team_members WHERE team_id = ?1 AND member_name = ?2",
            params![team_id, member_name],
        )?;
        Ok(())
    }

    /// Update a single member role within a team.
    pub fn update_team_member_role(&self, team_id: &str, member_name: &str, role: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE team_members SET role = ?1 WHERE team_id = ?2 AND member_name = ?3",
            params![role, team_id, member_name],
        )?;
        Ok(())
    }

    /// Remove a member from a channel by channel name. Used when removing a team member
    /// so their channel membership is cleaned up alongside the team membership.
    pub fn leave_channel(&self, channel_name: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if let Some(ch) = Self::get_channel_by_name_inner(&conn, channel_name)? {
            conn.execute(
                "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                params![ch.id, member_name],
            )?;
        }
        Ok(())
    }

    /// Return all members of a team ordered by name.
    pub fn get_team_members(&self, team_id: &str) -> Result<Vec<TeamMember>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT team_id, member_name, member_type, member_id, role, joined_at
                 FROM team_members WHERE team_id = ?1 ORDER BY member_name",
            )?
            .query_map(params![team_id], |row| {
                Ok(TeamMember {
                    team_id: row.get(0)?,
                    member_name: row.get(1)?,
                    member_type: row.get(2)?,
                    member_id: row.get(3)?,
                    role: row.get(4)?,
                    joined_at: parse_datetime(&row.get::<_, String>(5)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// List all teams an agent belongs to, along with their role in each team.
    pub fn get_teams_by_agent_name(&self, agent_name: &str) -> Result<Vec<TeamMembership>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT t.name, tm.role FROM team_members tm
                 JOIN teams t ON t.id = tm.team_id
                 WHERE tm.member_name = ?1 AND tm.member_type = 'agent'
                 ORDER BY t.name",
            )?
            .query_map(params![agent_name], |row| {
                Ok(TeamMembership {
                    team_name: row.get(0)?,
                    role: row.get(1)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Snapshot the current set of agent members into team_task_quorum for a new swarm task.
    /// The trigger_message_id is the message that kicked off the task.
    pub fn snapshot_swarm_quorum(&self, team_id: &str, trigger_message_id: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let member_names: Vec<String> = tx
            .prepare(
                "SELECT member_name FROM team_members
                 WHERE team_id = ?1 AND member_type = 'agent'
                 ORDER BY member_name",
            )?
            .query_map(params![team_id], |row| row.get(0))?
            .filter_map(|row| row.ok())
            .collect();
        tx.execute(
            "INSERT OR IGNORE INTO team_task_quorum (trigger_message_id, team_id, member_name)
             SELECT ?1, team_id, member_name FROM team_members
             WHERE team_id = ?2 AND member_type = 'agent'",
            params![trigger_message_id, team_id],
        )?;
        let last_event_id = Self::append_team_coordination_event_tx(
            &tx,
            team_id,
            "team.quorum_snapshot",
            Some("system"),
            None,
            Some("snapshot_swarm_quorum"),
            json!({
                "triggerMessageId": trigger_message_id,
                "memberNames": member_names,
            }),
        )?;
        tx.commit()?;
        let _ = self.event_tx.send(last_event_id);
        Ok(())
    }

    /// Record a signal (e.g. "READY") from an agent for an open swarm task quorum.
    ///
    /// Finds the earliest unresolved trigger for this team, inserts the signal, then checks
    /// whether all quorum members have now signalled. Returns `true` when consensus is reached
    /// and the quorum row is marked resolved.
    pub fn record_swarm_signal(
        &self,
        team_id: &str,
        member_name: &str,
        signal: &str,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // Find the earliest unresolved trigger_message_id for this team.
        let trigger_id: Option<String> = tx
            .prepare(
                "SELECT q.trigger_message_id FROM team_task_quorum q
                 JOIN messages m ON m.id = q.trigger_message_id
                 WHERE q.team_id = ?1 AND q.resolved_at IS NULL
                 ORDER BY m.created_at ASC
                 LIMIT 1",
            )?
            .query_row(params![team_id], |r| r.get(0))
            .ok();

        let trigger_id = match trigger_id {
            None => return Ok(false), // no open quorum — discard signal
            Some(id) => id,
        };

        // Only insert if member_name is in the quorum for this trigger; discard signals
        // from agents that joined after the quorum was snapshotted.
        let signal_id = Uuid::new_v4().to_string();
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO team_task_signals (id, team_id, trigger_message_id, member_name, signal)
             SELECT ?1, ?2, ?3, ?4, ?5
             WHERE EXISTS (
                 SELECT 1 FROM team_task_quorum
                 WHERE trigger_message_id = ?3 AND member_name = ?4
             )",
            params![signal_id, team_id, trigger_id, member_name, signal],
        )?;
        if inserted == 0 {
            return Ok(false); // non-quorum member, discard signal
        }

        let mut last_event_id = Self::append_team_coordination_event_tx(
            &tx,
            team_id,
            "team.quorum_signaled",
            Some(member_name),
            Some("agent"),
            Some("record_swarm_signal"),
            json!({
                "triggerMessageId": trigger_id,
                "memberName": member_name,
                "signal": signal,
            }),
        )?;

        // Check if quorum is now complete (all expected members have signalled).
        let quorum_size: i64 = tx.query_row(
            "SELECT COUNT(*) FROM team_task_quorum WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;
        let signal_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM team_task_signals WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;

        if signal_count >= quorum_size {
            tx.execute(
                "UPDATE team_task_quorum SET resolved_at = datetime('now')
                 WHERE trigger_message_id = ?1",
                params![trigger_id],
            )?;
            let member_names: Vec<String> = tx
                .prepare(
                    "SELECT member_name FROM team_task_quorum
                     WHERE trigger_message_id = ?1
                     ORDER BY member_name",
                )?
                .query_map(params![trigger_id], |row| row.get(0))?
                .filter_map(|row| row.ok())
                .collect();
            last_event_id = Self::append_team_coordination_event_tx(
                &tx,
                team_id,
                "team.quorum_reached",
                Some("system"),
                None,
                Some("record_swarm_signal"),
                json!({
                    "triggerMessageId": trigger_id,
                    "memberNames": member_names,
                    "signalCount": signal_count,
                }),
            )?;
            tx.commit()?;
            let _ = self.event_tx.send(last_event_id);
            return Ok(true);
        }

        tx.commit()?;
        let _ = self.event_tx.send(last_event_id);
        Ok(false)
    }
}
