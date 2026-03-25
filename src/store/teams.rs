use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

/// A named group of agents (and optional human observers) that collaborate on tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub display_name: String,
    /// Collaboration strategy: "swarm" (all agents decide together) or "leader_operators".
    pub collaboration_model: String,
    /// For leader_operators model, the agent designated as the leader.
    pub leader_agent_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A single member (agent or human) within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub team_id: String,
    pub member_name: String,
    pub member_type: String,
    /// Agent UUID or human username — always populated at insert.
    pub member_id: String,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// Summary of a team membership for use in agent system prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMembership {
    pub team_name: String,
    pub role: String,
}

/// Parse a Team row from the standard 6-column SELECT.
fn team_from_row(row: &rusqlite::Row) -> rusqlite::Result<Team> {
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        collaboration_model: row.get(3)?,
        leader_agent_name: row.get(4)?,
        created_at: parse_datetime(&row.get::<_, String>(5)?),
    })
}

impl Store {
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
            params![id, name, display_name, collaboration_model, leader_agent_name],
        )?;
        Ok(id)
    }

    /// Look up a team by its unique short name. Returns `None` if not found.
    pub fn get_team(&self, name: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
             FROM teams WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], team_from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Look up a team by its UUID. Returns `None` if not found.
    pub fn get_team_by_id(&self, id: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
             FROM teams WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], team_from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// List all teams ordered by name.
    pub fn list_teams(&self) -> Result<Vec<Team>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
                 FROM teams ORDER BY name",
            )?
            .query_map([], team_from_row)?
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
    pub fn add_team_member(
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
    pub fn remove_team_member(&self, team_id: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM team_members WHERE team_id = ?1 AND member_name = ?2",
            params![team_id, member_name],
        )?;
        Ok(())
    }

    /// Remove a member from a channel by channel name. Used when removing a team member
    /// so their channel membership is cleaned up alongside the team membership.
    pub fn leave_channel(&self, channel_name: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if let Some(ch) = Self::find_channel_by_name_inner(&conn, channel_name)? {
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
    pub fn list_teams_for_agent(&self, agent_name: &str) -> Result<Vec<TeamMembership>> {
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO team_task_quorum (trigger_message_id, team_id, member_name)
             SELECT ?1, team_id, member_name FROM team_members
             WHERE team_id = ?2 AND member_type = 'agent'",
            params![trigger_message_id, team_id],
        )?;
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
        let conn = self.conn.lock().unwrap();

        // Find the earliest unresolved trigger_message_id for this team.
        let trigger_id: Option<String> = conn
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
        let inserted = conn.execute(
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

        // Check if quorum is now complete (all expected members have signalled).
        let quorum_size: i64 = conn.query_row(
            "SELECT COUNT(*) FROM team_task_quorum WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;
        let signal_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM team_task_signals WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;

        if signal_count >= quorum_size {
            conn.execute(
                "UPDATE team_task_quorum SET resolved_at = datetime('now')
                 WHERE trigger_message_id = ?1",
                params![trigger_id],
            )?;
            return Ok(true);
        }

        Ok(false)
    }
}
