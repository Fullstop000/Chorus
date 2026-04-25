use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, ChannelType, Store};

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
    /// Create a new team and return its generated UUID.
    pub fn create_team(
        &self,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        Self::create_team_inner(
            &conn,
            None,
            name,
            display_name,
            collaboration_model,
            leader_agent_name,
        )
    }

    pub fn create_team_in_workspace(
        &self,
        workspace_id: &str,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        Self::create_team_inner(
            &conn,
            Some(workspace_id),
            name,
            display_name,
            collaboration_model,
            leader_agent_name,
        )
    }

    pub fn create_team_with_channel(
        &self,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<(String, String)> {
        self.create_team_with_channel_inner(
            None,
            name,
            display_name,
            collaboration_model,
            leader_agent_name,
        )
    }

    pub fn create_team_with_channel_in_workspace(
        &self,
        workspace_id: &str,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<(String, String)> {
        self.create_team_with_channel_inner(
            Some(workspace_id),
            name,
            display_name,
            collaboration_model,
            leader_agent_name,
        )
    }

    fn create_team_with_channel_inner(
        &self,
        workspace_id: Option<&str>,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<(String, String)> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let team_id = Self::create_team_inner(
            &tx,
            workspace_id,
            name,
            display_name,
            collaboration_model,
            leader_agent_name,
        )?;
        let channel_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO channels (id, workspace_id, name, description, channel_type, parent_channel_id)
             VALUES (?1, ?2, ?3, NULL, ?4, NULL)",
            params![
                channel_id,
                workspace_id,
                name,
                ChannelType::Team.as_api_str()
            ],
        )?;
        tx.commit()?;
        Ok((team_id, channel_id))
    }

    fn create_team_inner(
        conn: &rusqlite::Connection,
        workspace_id: Option<&str>,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO teams (id, workspace_id, name, display_name, collaboration_model, leader_agent_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                workspace_id,
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
             LEFT JOIN channels c ON c.name = t.name
                AND c.channel_type = 'team'
                AND c.archived = 0
                AND (c.workspace_id = t.workspace_id OR (c.workspace_id IS NULL AND t.workspace_id IS NULL))
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
             LEFT JOIN channels c ON c.name = t.name
                AND c.channel_type = 'team'
                AND c.archived = 0
                AND (c.workspace_id = t.workspace_id OR (c.workspace_id IS NULL AND t.workspace_id IS NULL))
             WHERE t.id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Team::from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// List all teams ordered by name.
    pub fn get_teams(&self) -> Result<Vec<Team>> {
        self.get_teams_inner(None)
    }

    pub fn get_teams_in_workspace(&self, workspace_id: &str) -> Result<Vec<Team>> {
        self.get_teams_inner(Some(workspace_id))
    }

    pub fn get_teams_for_workspace(&self, workspace_id: Option<&str>) -> Result<Vec<Team>> {
        self.get_teams_inner(workspace_id)
    }

    fn get_teams_inner(&self, workspace_id: Option<&str>) -> Result<Vec<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = "SELECT t.id, t.name, t.display_name, c.id, t.collaboration_model, t.leader_agent_name, t.created_at
             FROM teams t
             LEFT JOIN channels c ON c.name = t.name
                AND c.channel_type = 'team'
                AND c.archived = 0
                AND (c.workspace_id = t.workspace_id OR (c.workspace_id IS NULL AND t.workspace_id IS NULL))"
            .to_string();
        let mut values = Vec::new();
        if let Some(workspace_id) = workspace_id {
            sql.push_str(" WHERE t.workspace_id = ?1");
            values.push(workspace_id.to_string());
        } else {
            sql.push_str(" WHERE t.workspace_id IS NULL");
        }
        sql.push_str(" ORDER BY t.name");
        let rows = conn
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(values), Team::from_row)?
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

    /// Delete a team by id. Cascades to team_members via ON DELETE CASCADE.
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
    pub fn update_team_member_role(
        &self,
        team_id: &str,
        member_name: &str,
        role: &str,
    ) -> Result<()> {
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
}
