mod agents;
mod channels;
mod messages;
mod tasks;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection};
use tokio::sync::broadcast;

use crate::models::*;

pub struct Store {
    conn: Mutex<Connection>,
    msg_tx: broadcast::Sender<(String, String)>,
    data_dir: PathBuf,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::init_schema(&conn)?;
        Self::migrate_remove_spurious_dm_members(&conn)?;
        let (msg_tx, _) = broadcast::channel(256);
        Ok(Self {
            conn: Mutex::new(conn),
            msg_tx,
            data_dir: derive_data_dir(path),
        })
    }

    /// Return the configured server data directory that owns the SQLite file.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the directory used to persist uploaded attachments.
    pub fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join("attachments")
    }

    /// Return the directory that contains per-agent workspaces.
    pub fn agents_dir(&self) -> PathBuf {
        self.data_dir.join("agents")
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS channels (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                channel_type TEXT NOT NULL DEFAULT 'channel',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS channel_members (
                channel_id TEXT NOT NULL,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                last_read_seq INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (channel_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                thread_parent_id TEXT,
                sender_name TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                seq INTEGER NOT NULL,
                UNIQUE(channel_id, seq)
            );
            CREATE TABLE IF NOT EXISTS message_attachments (
                message_id TEXT NOT NULL,
                attachment_id TEXT NOT NULL,
                PRIMARY KEY (message_id, attachment_id)
            );
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                display_name TEXT NOT NULL,
                description TEXT,
                runtime TEXT NOT NULL,
                model TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'inactive',
                session_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS humans (
                name TEXT PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                task_number INTEGER NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'todo',
                claimed_by TEXT,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(channel_id, task_number)
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                stored_path TEXT NOT NULL,
                uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )?;
        Ok(())
    }

    /// One-time migration: remove agents that were incorrectly added to DM channels
    /// via `handle_create_agent`. A DM channel `dm-X-Y` must have exactly two members
    /// whose sorted names form the channel name. All other members are spurious.
    fn migrate_remove_spurious_dm_members(conn: &Connection) -> Result<()> {
        let dm_channels: Vec<(String, String)> = conn
            .prepare("SELECT id, name FROM channels WHERE channel_type = 'dm'")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (channel_id, channel_name) in dm_channels {
            let members: Vec<String> = conn
                .prepare("SELECT member_name FROM channel_members WHERE channel_id = ?1")?
                .query_map(params![channel_id], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            if members.len() <= 2 {
                continue;
            }

            // Find the pair (m1, m2) whose sorted join equals the channel name.
            let mut correct: Option<(String, String)> = None;
            'outer: for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let mut pair = [members[i].as_str(), members[j].as_str()];
                    pair.sort_unstable();
                    if format!("dm-{}-{}", pair[0], pair[1]) == channel_name {
                        correct = Some((members[i].clone(), members[j].clone()));
                        break 'outer;
                    }
                }
            }

            if let Some((m1, m2)) = correct {
                let removed = conn.execute(
                    "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name NOT IN (?2, ?3)",
                    params![channel_id, m1, m2],
                )?;
                if removed > 0 {
                    tracing::info!(channel = %channel_name, removed, "removed spurious members from DM channel");
                }
            }
        }
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.msg_tx.subscribe()
    }

    // ── Sender type lookup ──

    pub fn lookup_sender_type(&self, name: &str) -> Result<Option<SenderType>> {
        let conn = self.conn.lock().unwrap();
        Self::lookup_sender_type_inner(&conn, name)
    }

    fn lookup_sender_type_inner(conn: &Connection, name: &str) -> Result<Option<SenderType>> {
        let agent_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if agent_count > 0 {
            return Ok(Some(SenderType::Agent));
        }
        let human_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM humans WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if human_count > 0 {
            return Ok(Some(SenderType::Human));
        }
        Ok(None)
    }

    // ── Server info ──

    pub fn get_server_info(&self, for_agent: &str) -> Result<ServerInfo> {
        let conn = self.conn.lock().unwrap();

        let mut ch_stmt = conn.prepare(
            "SELECT c.name, c.description, EXISTS(SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.member_name = ?1) as joined FROM channels c WHERE c.channel_type = 'channel' ORDER BY c.name",
        )?;
        let channels: Vec<ChannelInfo> = ch_stmt
            .query_map(params![for_agent], |row| {
                Ok(ChannelInfo {
                    name: row.get(0)?,
                    description: row.get(1)?,
                    joined: row.get::<_, i64>(2)? > 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut ag_stmt = conn.prepare(
            "SELECT name, display_name, description, runtime, model, status, session_id FROM agents ORDER BY name",
        )?;
        let agents: Vec<AgentInfo> = ag_stmt
            .query_map([], |row| {
                Ok(AgentInfo {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    description: row.get(2)?,
                    runtime: row.get(3)?,
                    model: row.get(4)?,
                    status: row.get(5)?,
                    session_id: row.get(6)?,
                    activity: None,
                    activity_detail: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut hu_stmt = conn.prepare("SELECT name FROM humans ORDER BY name")?;
        let humans: Vec<HumanInfo> = hu_stmt
            .query_map([], |row| Ok(HumanInfo { name: row.get(0)? }))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(ServerInfo {
            channels,
            agents,
            humans,
        })
    }

    // ── Unread summary ──

    pub fn get_unread_summary(&self, agent_name: &str) -> Result<HashMap<String, i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.name, COUNT(m.id)
             FROM channel_members cm
             JOIN channels c ON cm.channel_id = c.id
             JOIN messages m ON m.channel_id = cm.channel_id AND m.seq > cm.last_read_seq AND m.thread_parent_id IS NULL
             WHERE cm.member_name = ?1
             GROUP BY c.name",
        )?;
        let rows = stmt
            .query_map(params![agent_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|r| r.ok());

        let mut map = HashMap::new();
        for (name, count) in rows {
            map.insert(name, count);
        }
        Ok(map)
    }
}

/// Derive the server data directory from the SQLite path.
fn derive_data_dir(path: &str) -> PathBuf {
    if path == ":memory:" {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        return PathBuf::from(home).join(".chorus");
    }

    Path::new(path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

// ── Helpers (shared across submodules) ──

pub(crate) fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc())
        .unwrap_or_else(|_| chrono::Utc::now())
}

pub(crate) fn sender_type_str(st: SenderType) -> &'static str {
    match st {
        SenderType::Human => "human",
        SenderType::Agent => "agent",
    }
}

pub(crate) fn parse_sender_type(s: &str) -> SenderType {
    match s {
        "agent" => SenderType::Agent,
        _ => SenderType::Human,
    }
}

pub(crate) fn parse_agent_status(s: &str) -> AgentStatus {
    match s {
        "active" => AgentStatus::Active,
        "sleeping" => AgentStatus::Sleeping,
        _ => AgentStatus::Inactive,
    }
}

/// Parse a Channel row from the standard 5-column SELECT.
pub(crate) fn channel_from_row(row: &rusqlite::Row) -> rusqlite::Result<Channel> {
    Ok(Channel {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        channel_type: match row.get::<_, String>(3)?.as_str() {
            "dm" => ChannelType::Dm,
            _ => ChannelType::Channel,
        },
        created_at: parse_datetime(&row.get::<_, String>(4)?),
    })
}
