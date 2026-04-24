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
    migrate_drop_agents_status_and_session_id_columns(conn)?;
    migrate_add_parent_channel_id(conn)?;
    migrate_add_task_sub_channel_id(conn)?;
    migrate_create_task_proposals_table(conn)?;
    migrate_add_task_proposal_snapshot_columns(conn)?;
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

/// Add nullable `parent_channel_id` column to `channels` if missing.
fn migrate_add_parent_channel_id(conn: &Connection) -> Result<()> {
    let exists: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('channels') WHERE name = 'parent_channel_id'")?
        .exists([])?;
    if !exists {
        conn.execute(
            "ALTER TABLE channels ADD COLUMN parent_channel_id TEXT REFERENCES channels(id)",
            [],
        )?;
        tracing::info!("migration: added parent_channel_id column to channels");
    }
    Ok(())
}

/// Add nullable `sub_channel_id` column to `tasks` if missing, then backfill
/// sub-channels for pre-existing tasks. Idempotent: after backfill runs once,
/// no task row has `sub_channel_id IS NULL`, so the SELECT returns zero rows.
fn migrate_add_task_sub_channel_id(conn: &Connection) -> Result<()> {
    let exists: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'sub_channel_id'")?
        .exists([])?;
    if !exists {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN sub_channel_id TEXT REFERENCES channels(id)",
            [],
        )?;
        tracing::info!("migration: added sub_channel_id column to tasks");
    }

    // Backfill: every existing task gets a sub-channel matching the new primitive.
    // Fetch orphan rows first (outside tx) to avoid borrowing conn twice.
    let orphans: Vec<(String, String, String, i64, String)> = conn
        .prepare(
            "SELECT t.id, c.id, c.name, t.task_number, t.created_by \
             FROM tasks t \
             JOIN channels c ON c.id = t.channel_id \
             WHERE t.sub_channel_id IS NULL",
        )?
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if orphans.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    for (task_id, parent_id, parent_name, task_number, creator_name) in orphans {
        let sub_id = uuid::Uuid::new_v4().to_string();
        // Preferred name mirrors what live `create_tasks` would spawn. Channel
        // names are user-controlled and globally unique, so an existing row
        // like `eng__task-1` (created manually before upgrade) would fail this
        // INSERT on the UNIQUE constraint and block startup. Fall back to a
        // uuid-suffixed name in that case — uglier but guaranteed collision-free
        // and still traceable via `tasks.sub_channel_id → channels.id`.
        let preferred = format!("{}__task-{}", parent_name, task_number);
        let name_taken: bool = tx
            .prepare("SELECT 1 FROM channels WHERE name = ?1")?
            .exists(rusqlite::params![preferred])?;
        let sub_name = if name_taken {
            let short = sub_id.split('-').next().unwrap_or("bk");
            let fallback = format!("{}__task-{}-{}", parent_name, task_number, short);
            tracing::warn!(
                task_id = %task_id,
                preferred = %preferred,
                fallback = %fallback,
                "backfill name collision — using uuid-suffixed fallback"
            );
            fallback
        } else {
            preferred
        };
        let creator_type = {
            let is_agent: bool = tx
                .prepare("SELECT 1 FROM agents WHERE name = ?1")?
                .exists(rusqlite::params![creator_name])?;
            if is_agent {
                "agent"
            } else {
                "human"
            }
        };
        tx.execute(
            "INSERT INTO channels (id, name, description, channel_type, parent_channel_id) \
             VALUES (?1, ?2, NULL, 'task', ?3)",
            rusqlite::params![sub_id, sub_name, parent_id],
        )?;
        tx.execute(
            "INSERT INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
             VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![sub_id, creator_name, creator_type],
        )?;
        tx.execute(
            "UPDATE tasks SET sub_channel_id = ?1 WHERE id = ?2",
            rusqlite::params![sub_id, task_id],
        )?;
        tracing::info!(task_id = %task_id, sub_channel = %sub_name, "backfilled task sub-channel");
    }
    tx.commit()?;
    Ok(())
}

fn migrate_drop_agents_status_and_session_id_columns(conn: &Connection) -> Result<()> {
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(agents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    let has_status = cols.iter().any(|c| c == "status");
    let has_session_id = cols.iter().any(|c| c == "session_id");
    if !has_status && !has_session_id {
        return Ok(());
    }
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         BEGIN;
         CREATE TABLE agents_new (
            id TEXT PRIMARY KEY,
            name TEXT UNIQUE NOT NULL,
            display_name TEXT NOT NULL,
            description TEXT,
            system_prompt TEXT,
            runtime TEXT NOT NULL,
            model TEXT NOT NULL,
            reasoning_effort TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO agents_new (
            id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at
         )
            SELECT id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at
            FROM agents;
         DROP TABLE agents;
         ALTER TABLE agents_new RENAME TO agents;
         COMMIT;
         PRAGMA foreign_keys=ON;",
    )?;
    tracing::info!("migration: dropped agents.status and agents.session_id columns");
    Ok(())
}

fn migrate_create_task_proposals_table(conn: &Connection) -> Result<()> {
    // Fresh DBs get the table from schema.sql. For DBs opened before
    // this feature landed, create it here. Idempotent via IF NOT EXISTS.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_proposals (
            id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
            proposed_by TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'dismissed')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            accepted_task_number INTEGER,
            accepted_sub_channel_id TEXT REFERENCES channels(id) ON DELETE SET NULL,
            resolved_by TEXT,
            resolved_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_task_proposals_channel_status
            ON task_proposals(channel_id, status);",
    )?;
    tracing::info!("task_proposals table ensured");
    Ok(())
}

/// v2: add snapshot columns + source-message pointer to `task_proposals`.
///
/// SQLite's `ALTER TABLE ADD COLUMN` cannot add a CHECK constraint, and we
/// need the all-or-nothing invariant across the 5 snapshot fields enforced on
/// migrated DBs (not just fresh installs). Use SQLite's canonical
/// table-rebuild pattern: create a new table with the full target shape, copy
/// the v1 rows (which have NULL snapshot columns), drop the old table, rename.
///
/// Rebuild > app-layer assertion: a runtime bug or external writer could land
/// partial-snapshot rows on a migrated DB without the CHECK. Paying the
/// one-time migration cost keeps the invariant durable everywhere downstream.
///
/// Idempotent via a `snapshot_content` column-existence probe.
fn migrate_add_task_proposal_snapshot_columns(conn: &Connection) -> Result<()> {
    let has_column: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('task_proposals') \
         WHERE name = 'snapshot_content'",
        [],
        |row| row.get::<_, i64>(0),
    )? > 0;
    if has_column {
        return Ok(());
    }
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys=OFF;
        BEGIN;
        CREATE TABLE task_proposals_new (
          id TEXT PRIMARY KEY,
          channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
          proposed_by TEXT NOT NULL,
          title TEXT NOT NULL,
          status TEXT NOT NULL DEFAULT 'pending'
              CHECK (status IN ('pending', 'accepted', 'dismissed')),
          created_at TEXT NOT NULL DEFAULT (datetime('now')),
          accepted_task_number INTEGER,
          accepted_sub_channel_id TEXT REFERENCES channels(id) ON DELETE SET NULL,
          resolved_by TEXT,
          resolved_at TEXT,
          -- v2 snapshot fields — see schema.sql for full rationale.
          -- source_message_id: navigation pointer; independently nullable.
          -- snapshot_*: immutable copy of the source message at propose-time,
          -- all-or-nothing (enforced by CHECK below). Context consistency for
          -- the per-task ACP session.
          source_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
          snapshot_sender_name TEXT,
          snapshot_sender_type TEXT,
          snapshot_content TEXT,
          snapshot_created_at TEXT,
          snapshotted_at TEXT,
          CHECK (
            (snapshot_sender_name IS NULL AND snapshot_sender_type IS NULL AND snapshot_content IS NULL
             AND snapshot_created_at IS NULL AND snapshotted_at IS NULL)
            OR
            (snapshot_sender_name IS NOT NULL AND snapshot_sender_type IS NOT NULL AND snapshot_content IS NOT NULL
             AND snapshot_created_at IS NOT NULL AND snapshotted_at IS NOT NULL)
          )
        );
        INSERT INTO task_proposals_new (
          id, channel_id, proposed_by, title, status, created_at,
          accepted_task_number, accepted_sub_channel_id, resolved_by, resolved_at
        )
        SELECT
          id, channel_id, proposed_by, title, status, created_at,
          accepted_task_number, accepted_sub_channel_id, resolved_by, resolved_at
        FROM task_proposals;
        DROP TABLE task_proposals;
        ALTER TABLE task_proposals_new RENAME TO task_proposals;
        CREATE INDEX idx_task_proposals_channel_status ON task_proposals(channel_id, status);
        COMMIT;
        PRAGMA foreign_keys=ON;
        "#,
    )?;
    tracing::info!("migration: added task_proposals snapshot columns");
    Ok(())
}

#[cfg(test)]
mod backfill_tests {
    use super::*;
    use rusqlite::Connection;

    fn bootstrap_pre_migration_db_with_creator(
        creator_name: &str,
        creator_table: &str,
    ) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Bare-minimum pre-migration schema — channels and tasks WITHOUT the new columns.
        conn.execute_batch(
            "CREATE TABLE channels (
                id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, description TEXT,
                channel_type TEXT NOT NULL DEFAULT 'channel',
                archived INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE channel_members (
                channel_id TEXT NOT NULL, member_name TEXT NOT NULL,
                member_type TEXT NOT NULL, last_read_seq INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (channel_id, member_name)
             );
             CREATE TABLE tasks (
                id TEXT PRIMARY KEY, channel_id TEXT NOT NULL,
                task_number INTEGER NOT NULL, title TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'todo', claimed_by TEXT,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(channel_id, task_number)
             );
             CREATE TABLE agents (name TEXT PRIMARY KEY);
             CREATE TABLE humans (name TEXT PRIMARY KEY);
             INSERT INTO channels (id, name) VALUES ('ch1', 'eng');",
        )
        .unwrap();
        conn.execute(
            &format!("INSERT INTO {} (name) VALUES (?1)", creator_table),
            rusqlite::params![creator_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, channel_id, task_number, title, created_by)
                 VALUES ('t1', 'ch1', 1, 'legacy task', ?1)",
            rusqlite::params![creator_name],
        )
        .unwrap();
        conn
    }

    fn bootstrap_pre_migration_db() -> Connection {
        bootstrap_pre_migration_db_with_creator("alice", "humans")
    }

    #[test]
    fn backfill_spawns_sub_channel_for_legacy_task() {
        let conn = bootstrap_pre_migration_db();
        migrate_add_parent_channel_id(&conn).unwrap();
        migrate_add_task_sub_channel_id(&conn).unwrap();

        let sub_id: String = conn
            .query_row(
                "SELECT sub_channel_id FROM tasks WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sub_name: String = conn
            .query_row("SELECT name FROM channels WHERE id = ?1", [&sub_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sub_name, "eng__task-1");
        let is_member: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = 'alice'",
                [&sub_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(is_member, 1);
        let parent_channel_id: Option<String> = conn
            .query_row(
                "SELECT parent_channel_id FROM channels WHERE id = ?1",
                [&sub_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent_channel_id.as_deref(), Some("ch1"));
    }

    #[test]
    fn backfill_uses_agent_member_type_when_creator_is_agent() {
        let conn = bootstrap_pre_migration_db_with_creator("bot", "agents");
        migrate_add_parent_channel_id(&conn).unwrap();
        migrate_add_task_sub_channel_id(&conn).unwrap();

        let sub_id: String = conn
            .query_row(
                "SELECT sub_channel_id FROM tasks WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let member_type: String = conn
            .query_row(
                "SELECT member_type FROM channel_members WHERE channel_id = ?1 AND member_name = 'bot'",
                [&sub_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member_type, "agent");
    }

    #[test]
    fn backfill_tolerates_name_collision() {
        // Regression: if a user manually created a channel with the exact
        // preferred backfill name before upgrading, the backfill INSERT would
        // hit the UNIQUE(name) constraint and crash startup. Must fall back
        // to a uuid-suffixed name and still link the task.
        let conn = bootstrap_pre_migration_db();
        // Pre-squat the preferred name as a regular channel.
        conn.execute(
            "INSERT INTO channels (id, name, channel_type) VALUES ('squat', 'eng__task-1', 'channel')",
            [],
        )
        .unwrap();

        migrate_add_parent_channel_id(&conn).unwrap();
        migrate_add_task_sub_channel_id(&conn).unwrap();

        // Task 1 still got a sub-channel (fallback name), and the pre-squatted
        // regular channel is untouched.
        let sub_id: String = conn
            .query_row(
                "SELECT sub_channel_id FROM tasks WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sub_name: String = conn
            .query_row("SELECT name FROM channels WHERE id = ?1", [&sub_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_ne!(
            sub_name, "eng__task-1",
            "must pick a different name when preferred is taken"
        );
        assert!(
            sub_name.starts_with("eng__task-1-"),
            "fallback name should preserve the task-number prefix, got: {}",
            sub_name
        );

        // Pre-squatted channel is still a regular channel, not a task child.
        let squat_type: String = conn
            .query_row(
                "SELECT channel_type FROM channels WHERE id = 'squat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(squat_type, "channel");
    }

    #[test]
    fn backfill_is_idempotent() {
        let conn = bootstrap_pre_migration_db();
        migrate_add_parent_channel_id(&conn).unwrap();
        migrate_add_task_sub_channel_id(&conn).unwrap();
        // Running it again must not spawn a second sub-channel.
        migrate_add_task_sub_channel_id(&conn).unwrap();
        let task_channels: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE channel_type = 'task'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(task_channels, 1);
    }
}

#[cfg(test)]
mod snapshot_migration_tests {
    use super::*;
    use rusqlite::Connection;

    /// Bootstrap a Connection with the pre-v2 `task_proposals` schema (plus the
    /// minimum referenced tables). Mirrors what a DB opened before this task
    /// landed would look like. Deliberately does NOT call `run_migrations` —
    /// that would apply the v2 migration and make these tests tautological.
    fn open_test_db_v1_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE channels (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                channel_type TEXT NOT NULL DEFAULT 'channel',
                archived INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                parent_channel_id TEXT REFERENCES channels(id)
             );
             CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                sender_name TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                sender_deleted INTEGER NOT NULL DEFAULT 0,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                seq INTEGER NOT NULL,
                UNIQUE(channel_id, seq)
             );
             CREATE TABLE task_proposals (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
                proposed_by TEXT NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'accepted', 'dismissed')),
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                accepted_task_number INTEGER,
                accepted_sub_channel_id TEXT REFERENCES channels(id) ON DELETE SET NULL,
                resolved_by TEXT,
                resolved_at TEXT
             );
             CREATE INDEX idx_task_proposals_channel_status ON task_proposals(channel_id, status);
             INSERT INTO channels (id, name) VALUES ('ch', 'eng');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn migration_add_snapshot_columns_is_idempotent() {
        let conn = open_test_db_v1_schema();
        migrate_add_task_proposal_snapshot_columns(&conn).unwrap();
        migrate_add_task_proposal_snapshot_columns(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('task_proposals') WHERE name = 'snapshot_content'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn migrated_db_rejects_partial_snapshot_insert() {
        let conn = open_test_db_v1_schema();
        // Pre-migration row with NULL snapshot (survives migration).
        conn.execute(
            "INSERT INTO task_proposals (id, channel_id, proposed_by, title, status, created_at)
             VALUES ('old-prop', 'ch', 'claude', 't', 'pending', datetime('now'))",
            [],
        )
        .unwrap();
        migrate_add_task_proposal_snapshot_columns(&conn).unwrap();

        // Attempt a partial snapshot (content set, other snapshot fields NULL).
        // The CHECK must reject it on migrated DBs just as on fresh installs.
        let res = conn.execute(
            "INSERT INTO task_proposals (id, channel_id, proposed_by, title, status, created_at,
                snapshot_content) VALUES ('p1', 'ch', 'claude', 't', 'pending',
                datetime('now'), 'partial')",
            [],
        );
        assert!(
            res.is_err(),
            "migrated DB must enforce the same CHECK as fresh DB"
        );
    }

    #[test]
    fn legacy_v1_row_survives_migration_with_all_columns_preserved() {
        let conn = open_test_db_v1_schema();
        // Seed a v1 row exercising every column including terminal-state fields.
        // `accepted_sub_channel_id` references channels(id), so seed that row too.
        conn.execute(
            "INSERT INTO channels (id, name) VALUES ('sub-ch-7', 'eng__task-7')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_proposals (
                id, channel_id, proposed_by, title, status, created_at,
                accepted_task_number, accepted_sub_channel_id,
                resolved_by, resolved_at
             ) VALUES (
                'legacy-1', 'ch', 'claude', 'legacy title', 'accepted',
                '2026-04-20T10:00:00Z', 7, 'sub-ch-7',
                'alice', '2026-04-20T10:05:00Z'
             )",
            [],
        )
        .unwrap();

        migrate_add_task_proposal_snapshot_columns(&conn).unwrap();

        #[allow(clippy::type_complexity)]
        let (
            channel_id,
            proposed_by,
            title,
            status,
            created_at,
            task_no,
            sub_ch,
            resolved_by,
            resolved_at,
            src_msg,
            snap_content,
            snap_sender,
            snap_sender_type,
            snap_created,
            snapped_at,
        ): (
            String,
            String,
            String,
            String,
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT channel_id, proposed_by, title, status, created_at,
                        accepted_task_number, accepted_sub_channel_id,
                        resolved_by, resolved_at,
                        source_message_id, snapshot_content, snapshot_sender_name,
                        snapshot_sender_type, snapshot_created_at, snapshotted_at
                 FROM task_proposals WHERE id = 'legacy-1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                },
            )
            .unwrap();

        // Every v1 field preserved:
        assert_eq!(channel_id, "ch");
        assert_eq!(proposed_by, "claude");
        assert_eq!(title, "legacy title");
        assert_eq!(status, "accepted");
        assert_eq!(created_at, "2026-04-20T10:00:00Z");
        assert_eq!(task_no, Some(7));
        assert_eq!(sub_ch.as_deref(), Some("sub-ch-7"));
        assert_eq!(resolved_by.as_deref(), Some("alice"));
        assert_eq!(resolved_at.as_deref(), Some("2026-04-20T10:05:00Z"));
        // v2 snapshot columns absent, consistent with legacy rows:
        assert!(src_msg.is_none());
        assert!(snap_content.is_none());
        assert!(snap_sender.is_none());
        assert!(snap_sender_type.is_none());
        assert!(snap_created.is_none());
        assert!(snapped_at.is_none());

        // Index preserved:
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_task_proposals_channel_status'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }
}
