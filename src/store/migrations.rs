use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

pub(super) fn run_migrations(conn: &Connection) -> Result<()> {
    migrate_remove_spurious_dm_members(conn)?;
    migrate_drop_legacy_event_tables(conn)?;
    migrate_remove_legacy_shared_memory_channel(conn)?;
    migrate_inbox_read_state(conn)?;
    migrate_add_system_prompt_column(conn)?;
    migrate_add_run_id_to_messages(conn)?;
    migrate_add_trace_summary_to_messages(conn)?;
    migrate_create_trace_events_table(conn)?;
    migrate_add_display_name_to_humans(conn)?;
    migrate_create_agent_sessions_table(conn)?;
    migrate_copy_session_ids_to_agent_sessions(conn)?;
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
            .query_map(rusqlite::params![channel_id], |row| row.get(0))?
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
                rusqlite::params![channel_id, m1, m2],
            )?;
            if removed > 0 {
                tracing::info!(channel = %channel_name, removed, "removed spurious members from DM channel");
            }
        }
    }
    Ok(())
}

/// Remove deprecated durable event log tables (replaced by in-memory stream fanout only).
fn migrate_drop_legacy_event_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS events;
         DROP TABLE IF EXISTS streams;",
    )?;
    Ok(())
}

/// Remove the retired `#shared-memory` system channel from upgraded stores.
fn migrate_remove_legacy_shared_memory_channel(conn: &Connection) -> Result<()> {
    let legacy_channel_id: Option<String> = conn
        .query_row(
            "SELECT id FROM channels WHERE name = 'shared-memory' AND channel_type = 'system'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    let Some(channel_id) = legacy_channel_id else {
        return Ok(());
    };

    conn.execute(
        "DELETE FROM inbox_read_state WHERE conversation_id = ?1",
        rusqlite::params![channel_id],
    )?;
    conn.execute(
        "DELETE FROM message_attachments
         WHERE message_id IN (SELECT id FROM messages WHERE channel_id = ?1)",
        rusqlite::params![channel_id],
    )?;
    conn.execute(
        "DELETE FROM messages WHERE channel_id = ?1",
        rusqlite::params![channel_id],
    )?;
    conn.execute(
        "DELETE FROM tasks WHERE channel_id = ?1",
        rusqlite::params![channel_id],
    )?;
    conn.execute(
        "DELETE FROM channel_members WHERE channel_id = ?1",
        rusqlite::params![channel_id],
    )?;
    conn.execute(
        "DELETE FROM channels WHERE id = ?1",
        rusqlite::params![channel_id],
    )?;
    tracing::info!("removed legacy shared-memory system channel");
    Ok(())
}

/// Add `system_prompt` column to agents table for rich template prompts.
fn migrate_add_system_prompt_column(conn: &Connection) -> Result<()> {
    let has_column: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('agents') WHERE name = 'system_prompt'")?
        .query_map([], |_| Ok(true))?
        .any(|r| r.unwrap_or(false));
    if !has_column {
        conn.execute("ALTER TABLE agents ADD COLUMN system_prompt TEXT", [])?;
        tracing::info!("added system_prompt column to agents table");
    }
    Ok(())
}

pub(super) fn migrate_inbox_read_state(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO inbox_read_state (
            conversation_id, member_name, member_type, last_read_seq, last_read_message_id
         )
         SELECT
            cm.channel_id,
            cm.member_name,
            cm.member_type,
            cm.last_read_seq,
            (
                SELECT m.id
                FROM messages m
                WHERE m.channel_id = cm.channel_id
                  AND m.seq = cm.last_read_seq
                LIMIT 1
            )
         FROM channel_members cm",
        [],
    )?;
    Ok(())
}

/// Add run_id column to messages for Telescope trace correlation.
fn migrate_add_run_id_to_messages(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(messages)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|col| col == "run_id");
    if !has_column {
        conn.execute_batch("ALTER TABLE messages ADD COLUMN run_id TEXT")?;
        tracing::info!("migration: added run_id column to messages");
    }
    Ok(())
}

/// Add trace_summary column to messages for collapsed Telescope rendering.
fn migrate_add_trace_summary_to_messages(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(messages)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|col| col == "trace_summary");
    if !has_column {
        conn.execute_batch("ALTER TABLE messages ADD COLUMN trace_summary TEXT")?;
        tracing::info!("migration: added trace_summary column to messages");
    }
    Ok(())
}

/// Create the trace_events table for Telescope trace persistence.
fn migrate_create_trace_events_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS trace_events (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            timestamp_ms INTEGER NOT NULL,
            kind TEXT NOT NULL,
            data TEXT NOT NULL,
            UNIQUE(run_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_trace_events_run_seq ON trace_events(run_id, seq);",
    )?;
    Ok(())
}

/// Add display_name column to humans table for user-friendly names.
fn migrate_add_display_name_to_humans(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(humans)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|col| col == "display_name");
    if !has_column {
        conn.execute_batch("ALTER TABLE humans ADD COLUMN display_name TEXT")?;
        tracing::info!("migration: added display_name column to humans");
    }
    Ok(())
}

/// Create the agent_sessions table that backs the 1:N Agent:Session relationship.
fn migrate_create_agent_sessions_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL,
            runtime TEXT NOT NULL,
            is_active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(agent_id, session_id)
         );
         CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent_active
            ON agent_sessions(agent_id, is_active);",
    )?;
    Ok(())
}

fn migrate_copy_session_ids_to_agent_sessions(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(agents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|col| col == "session_id");
    if !has_column {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO agent_sessions (agent_id, session_id, runtime, is_active, created_at, last_used_at)
         SELECT id, session_id, runtime, 1, created_at, datetime('now')
         FROM agents
         WHERE session_id IS NOT NULL AND session_id != ''",
        [],
    )?;
    tracing::info!("migration: copied agents.session_id values into agent_sessions");
    Ok(())
}
