use anyhow::{anyhow, Result};
use rusqlite::params;
use uuid::Uuid;

use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::*;
use crate::store::Store;

impl Store {
    /// Resolve a `#channel` or `dm:@name` target into a `channel_id`.
    ///
    /// Legacy thread forms (`#channel:shortid`, `dm:@name:shortid`) are rejected
    /// explicitly so stale clients do not silently misroute messages into a DM
    /// with a peer whose name happens to match the full suffix.
    pub fn resolve_target(&self, target: &str, sender_name: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();

        if let Some(rest) = target.strip_prefix("dm:@") {
            if rest.contains(':') {
                return Err(anyhow!(
                    "thread targets are no longer supported (got: {})",
                    target
                ));
            }
            let other_name = rest;

            let mut names = [sender_name.to_string(), other_name.to_string()];
            names.sort();
            let dm_name = format!("dm-{}-{}", names[0], names[1]);

            let channel = match Self::get_channel_by_name_inner(&conn, &dm_name)? {
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

            Ok(channel.id)
        } else if let Some(rest) = target.strip_prefix('#') {
            if rest.contains(':') {
                return Err(anyhow!(
                    "thread targets are no longer supported (got: {})",
                    target
                ));
            }
            let channel = Self::get_channel_by_name_inner(&conn, rest)?
                .ok_or_else(|| anyhow!("channel not found: {}", rest))?;
            Ok(channel.id)
        } else {
            Err(anyhow!("invalid target format: {}", target))
        }
    }
}
