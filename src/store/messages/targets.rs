use anyhow::{anyhow, Result};
use rusqlite::params;
use uuid::Uuid;

use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::*;
use crate::store::Store;

impl Store {
    /// Resolve a `#channel`, `#channel:msgid`, `dm:@name`, or `dm:@name:msgid` target
    /// into `(channel_id, thread_parent_id)`.
    pub fn resolve_target(
        &self,
        target: &str,
        sender_name: &str,
    ) -> Result<(String, Option<String>)> {
        let conn = self.conn.lock().unwrap();

        if let Some(rest) = target.strip_prefix("dm:@") {
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let other_name = parts[0];
            let thread_short = parts.get(1).copied();

            let mut names = [sender_name.to_string(), other_name.to_string()];
            names.sort();
            let dm_name = format!("dm-{}-{}", names[0], names[1]);

            let channel = match Self::find_channel_by_name_inner(&conn, &dm_name)? {
                Some(ch) => ch,
                None => {
                    let id = Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO channels (id, name, channel_type) VALUES (?1, ?2, 'dm')",
                        params![id, dm_name],
                    )?;
                    let sender_mt = Self::lookup_sender_type_inner(&conn, sender_name)?
                        .map(SenderType::as_str)
                        .unwrap_or("agent");
                    let other_mt = Self::lookup_sender_type_inner(&conn, other_name)?
                        .map(SenderType::as_str)
                        .unwrap_or("human");
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, sender_name, sender_mt],
                    )?;
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, other_name, other_mt],
                    )?;
                    Channel {
                        id,
                        name: dm_name,
                        description: None,
                        channel_type: ChannelType::Dm,
                        created_at: chrono::Utc::now(),
                    }
                }
            };

            let thread_parent_id = thread_short.and_then(|short| {
                conn.query_row(
                    "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                    params![channel.id, format!("{}%", short)],
                    |row| row.get(0),
                )
                .ok()
            });

            Ok((channel.id, thread_parent_id))
        } else if let Some(rest) = target.strip_prefix('#') {
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let channel_name = parts[0];
            let thread_short = parts.get(1).copied();

            let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
                .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

            let thread_parent_id = thread_short.and_then(|short| {
                conn.query_row(
                    "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                    params![channel.id, format!("{}%", short)],
                    |row| row.get(0),
                )
                .ok()
            });

            Ok((channel.id, thread_parent_id))
        } else {
            Err(anyhow!("invalid target format: {}", target))
        }
    }
}
