use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::*;
use crate::store::Store;

impl Store {
    /// Resolve a `#channel` or `dm:@name` target into a `channel_id`.
    pub fn resolve_target(&self, target: &str, sender_id: &str) -> Result<String> {
        self.resolve_target_for_workspace(target, sender_id, None)
    }

    /// Resolve a target inside an optional workspace context. Compatibility
    /// callers without an explicit id are assigned to the active local
    /// workspace before any channel row is created or queried.
    pub fn resolve_target_for_workspace(
        &self,
        target: &str,
        sender_id: &str,
        workspace_id: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = match workspace_id {
            Some(workspace_id) => workspace_id.to_string(),
            None => Self::workspace_id_for_write_inner(&conn)?,
        };

        if let Some(rest) = target.strip_prefix("dm:@") {
            let other_lookup = rest;
            let sender_type = Self::lookup_sender_type_inner(&conn, sender_id)?
                .ok_or_else(|| anyhow!("sender not found: {sender_id}"))?;
            let (other_id, other_type) = Self::lookup_sender_ref_inner(&conn, other_lookup)?
                .ok_or_else(|| anyhow!("peer not found: {other_lookup}"))?;

            let mut participant_ids = [sender_id.to_string(), other_id.clone()];
            participant_ids.sort();
            let dm_name = format!("dm-{}-{}", participant_ids[0], participant_ids[1]);

            let channel = match Self::get_channel_by_workspace_and_name_inner(
                &conn,
                &workspace_id,
                &dm_name,
            )? {
                Some(ch) => ch,
                None => {
                    let id = Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO channels (id, workspace_id, name, channel_type) VALUES (?1, ?2, ?3, 'dm')",
                        params![id, workspace_id, dm_name],
                    )?;
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, sender_id, sender_type.as_str()],
                    )?;
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, other_id, other_type.as_str()],
                    )?;
                    Channel {
                        id,
                        workspace_id,
                        name: dm_name,
                        description: None,
                        channel_type: ChannelType::Dm,
                        created_at: chrono::Utc::now(),
                        parent_channel_id: None,
                    }
                }
            };

            Ok(channel.id)
        } else if let Some(rest) = target.strip_prefix('#') {
            let channel =
                Self::get_channel_by_workspace_and_name_inner(&conn, &workspace_id, rest)?
                    .ok_or_else(|| anyhow!("channel not found: {}", rest))?;
            Ok(channel.id)
        } else {
            Err(anyhow!("invalid target format: {}", target))
        }
    }

    pub(crate) fn lookup_sender_ref_inner(
        conn: &rusqlite::Connection,
        value: &str,
    ) -> Result<Option<(String, SenderType)>> {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM humans WHERE id = ?1 OR name = ?1",
                params![value],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some((id, SenderType::Human)));
        }
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM agents WHERE id = ?1 OR name = ?1",
                params![value],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some((id, SenderType::Agent)));
        }
        Ok(None)
    }
}
