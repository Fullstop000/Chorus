use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub(super) fn run_migrations(conn: &Connection) -> Result<()> {
    migrate_remove_spurious_dm_members(conn)?;
    migrate_event_stream_identity(conn)?;
    migrate_inbox_read_state(conn)?;
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

/// Backfill per-domain stream identity onto existing durable events and
/// ensure the lightweight stream cursor table reflects the latest position
/// for every known stream. Safe to run on every startup.
fn migrate_event_stream_identity(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String, String, Option<String>)> = conn
        .prepare(
            "SELECT event_id, scope_kind, scope_id, channel_id
             FROM events
             ORDER BY event_id ASC",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|row| row.ok())
        .collect();

    let mut stream_positions: HashMap<String, i64> = HashMap::new();
    let mut stream_meta: HashMap<String, (String, String, i64)> = HashMap::new();

    for (event_id, scope_kind, scope_id, channel_id) in rows {
        let (stream_id, stream_kind, aggregate_id) = crate::store::events::derive_stream_identity(
            &scope_kind,
            &scope_id,
            channel_id.as_deref(),
        );
        let next_pos = stream_positions.get(&stream_id).copied().unwrap_or(0) + 1;
        stream_positions.insert(stream_id.clone(), next_pos);
        stream_meta.insert(
            stream_id.clone(),
            (stream_kind.clone(), aggregate_id.clone(), next_pos),
        );
        conn.execute(
            "UPDATE events
             SET stream_id = ?1, stream_kind = ?2, stream_pos = ?3
             WHERE event_id = ?4",
            params![stream_id, stream_kind, next_pos, event_id],
        )?;
    }

    for (stream_id, (stream_kind, aggregate_id, current_pos)) in stream_meta {
        conn.execute(
            "INSERT INTO streams (stream_id, stream_kind, aggregate_id, current_pos)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(stream_id) DO UPDATE SET
                 stream_kind = excluded.stream_kind,
                 aggregate_id = excluded.aggregate_id,
                 current_pos = excluded.current_pos",
            params![stream_id, stream_kind, aggregate_id, current_pos],
        )?;
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
