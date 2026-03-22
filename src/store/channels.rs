use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::{channel_from_row, parse_sender_type, sender_type_str, Store};
use crate::models::*;

impl Store {
    pub fn create_channel(
        &self,
        name: &str,
        description: Option<&str>,
        channel_type: ChannelType,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let ct = match channel_type {
            ChannelType::Channel => "channel",
            ChannelType::Dm => "dm",
        };
        conn.execute(
            "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, description, ct],
        )?;
        Ok(id)
    }

    pub fn list_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE channel_type = 'channel' ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], channel_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_channel_by_name(&self, name: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_name_inner(&conn, name)
    }

    pub(crate) fn find_channel_by_name_inner(
        conn: &Connection,
        name: &str,
    ) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], channel_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn find_channel_by_id(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_id_inner(&conn, id)
    }

    pub(crate) fn find_channel_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], channel_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn join_channel(
        &self,
        channel_name: &str,
        member_name: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let mt = sender_type_str(member_type);
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
            params![channel.id, member_name, mt],
        )?;
        Ok(())
    }

    pub fn get_channel_members(&self, channel_id: &str) -> Result<Vec<ChannelMember>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, member_name, member_type, last_read_seq FROM channel_members WHERE channel_id = ?1",
        )?;
        let rows = stmt.query_map(params![channel_id], |row| {
            Ok(ChannelMember {
                channel_id: row.get(0)?,
                member_name: row.get(1)?,
                member_type: parse_sender_type(&row.get::<_, String>(2)?),
                last_read_seq: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn is_member(&self, channel_name: &str, member_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?;
        match channel {
            None => Ok(false),
            Some(ch) => {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                    params![ch.id, member_name],
                    |row| row.get(0),
                )?;
                Ok(count > 0)
            }
        }
    }
}
