//! Agent-related message operations.
//!
//! The `inbox` submodule holds the receive pipeline (`get_messages_for_agent`, read cursors).
//! This file holds delivery fan-out, activity listing, and thread access rules.

mod inbox;

use std::collections::BTreeSet;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::store::messages::ActivityMessage;
use crate::store::Store;

impl Store {
    /// Which agent members should receive delivery for this message. Top-level channel/DM fans out
    /// to every agent member except the sender. Thread replies only go to implicit participants.
    pub fn get_agent_message_recipients(
        &self,
        channel_id: &str,
        message_id: &str,
        sender_name: &str,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let thread_parent_id: Option<String> = conn.query_row(
            "SELECT thread_parent_id FROM messages WHERE id = ?1 AND channel_id = ?2",
            params![message_id, channel_id],
            |row| row.get(0),
        )?;

        if let Some(parent_id) = thread_parent_id {
            return Self::get_thread_agent_recipients_inner(
                &conn,
                channel_id,
                &parent_id,
                sender_name,
            );
        }

        let recipients = Self::get_channel_agent_members_inner(&conn, channel_id)?
            .into_iter()
            .filter(|member_name| member_name != sender_name)
            .collect();
        Ok(recipients)
    }

    pub fn mark_agent_messages_deleted(&self, name: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE messages SET sender_deleted = 1
             WHERE sender_type = 'agent' AND sender_name = ?1 AND sender_deleted = 0",
            params![name],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_agent_activity(&self, agent_name: &str, limit: i64) -> Result<Vec<ActivityMessage>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT m.id, m.seq, m.content, c.name, m.created_at
                 FROM messages m JOIN channels c ON c.id = m.channel_id
                 WHERE m.sender_name = ?1 ORDER BY m.created_at DESC LIMIT ?2",
            )?
            .query_map(params![agent_name, limit], |row| {
                Ok(ActivityMessage {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    content: row.get(2)?,
                    channel_name: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Agent member names for a channel (delivery policy builds on this list).
    pub(crate) fn get_channel_agent_members_inner(
        conn: &Connection,
        channel_id: &str,
    ) -> Result<Vec<String>> {
        let rows = conn
            .prepare(
                "SELECT member_name FROM channel_members WHERE channel_id = ?1 AND member_type = 'agent'",
            )?
            .query_map(params![channel_id], |row| row.get(0))?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    }

    /// Thread recipients: channel agents that may see the thread (parent author or prior replies).
    pub(crate) fn get_thread_agent_recipients_inner(
        conn: &Connection,
        channel_id: &str,
        parent_id: &str,
        sender_name: &str,
    ) -> Result<Vec<String>> {
        let channel_agents = Self::get_channel_agent_members_inner(conn, channel_id)?;
        let channel_agent_set: BTreeSet<String> = channel_agents.into_iter().collect();

        let mut recipients = BTreeSet::new();
        for agent_name in channel_agent_set {
            if agent_name == sender_name {
                continue;
            }
            if Self::agent_can_access_thread_inner(conn, channel_id, parent_id, &agent_name)? {
                recipients.insert(agent_name);
            }
        }

        Ok(recipients.into_iter().collect())
    }

    /// Agent can read a thread if it authored the parent or has already replied in that thread.
    pub(crate) fn agent_can_access_thread_inner(
        conn: &Connection,
        channel_id: &str,
        parent_id: &str,
        agent_name: &str,
    ) -> Result<bool> {
        let parent_author_matches: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM messages
             WHERE id = ?1 AND channel_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
            params![parent_id, channel_id, agent_name],
            |row| row.get(0),
        )?;
        if parent_author_matches > 0 {
            return Ok(true);
        }

        let prior_reply_matches: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM messages
             WHERE channel_id = ?1 AND thread_parent_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
            params![channel_id, parent_id, agent_name],
            |row| row.get(0),
        )?;
        Ok(prior_reply_matches > 0)
    }
}
