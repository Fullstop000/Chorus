use anyhow::{anyhow, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

// ── Types owned by this module ──

/// A single entry in the shared knowledge store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub key: String,
    pub value: String,
    pub tags: String,
    pub author_agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_context: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RememberRequest {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "channelContext")]
    pub channel_context: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RememberResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct RecallQuery {
    pub query: Option<String>,
    pub tags: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecallResponse {
    pub entries: Vec<KnowledgeEntry>,
}

/// Sanitize a raw FTS5 query string so agent-supplied input cannot inject FTS5 syntax
/// that causes a parse error. We escape double-quotes and strip bare operators.
fn sanitize_fts_query(raw: &str) -> String {
    // Wrap each word in double-quotes so FTS5 treats them as phrase literals.
    // This prevents injection of FTS5 operators like AND/OR/NOT/NEAR.
    raw.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Store {
    /// Write a new knowledge entry and return its ID.
    /// Caller is responsible for also posting the breadcrumb message to #shared-memory.
    pub fn remember(
        &self,
        key: &str,
        value: &str,
        tags: &str,
        author_agent_id: &str,
        channel_context: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO shared_knowledge (id, key, value, tags, author_agent_id, channel_context) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, key, value, tags, author_agent_id, channel_context],
        )?;
        Ok(id)
    }

    /// Search the shared knowledge store.
    ///
    /// - `query`: optional free-text search (FTS5 MATCH across key, value, tags)
    /// - `tags`: optional space-separated tag filter (each tag must appear in the entry's tags field)
    ///
    /// Returns up to 20 results ordered by recency.
    pub fn recall(&self, query: Option<&str>, tags: Option<&str>) -> Result<Vec<KnowledgeEntry>> {
        let conn = self.conn.lock().unwrap();

        // Build the SQL dynamically based on which filters are provided.
        //
        // Strategy:
        //  - If query is given: join shared_knowledge against knowledge_fts using FTS5 MATCH.
        //  - If tags filter is given: additionally filter by checking each tag token is in the
        //    tags field (simple INSTR check on space-separated tokens).
        //
        // We always order by shared_knowledge.rowid DESC (insertion order proxy for recency)
        // and cap at 20 results.

        let entries = if let Some(q) = query.filter(|s| !s.is_empty()) {
            let safe_query = sanitize_fts_query(q);
            let sql = "SELECT sk.id, sk.key, sk.value, sk.tags, sk.author_agent_id, \
                              sk.channel_context, sk.created_at \
                       FROM shared_knowledge sk \
                       JOIN knowledge_fts kf ON kf.rowid = sk.rowid \
                       WHERE knowledge_fts MATCH ?1 \
                       ORDER BY sk.rowid DESC LIMIT 20";
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| anyhow!("recall prepare error: {}", e))?;
            let rows = stmt
                .query_map(params![safe_query], knowledge_from_row)
                .map_err(|e| anyhow!("recall query error: {}", e))?;
            rows.filter_map(|r| r.ok()).collect::<Vec<_>>()
        } else {
            // No text query — fetch all, then filter by tags below.
            let sql = "SELECT id, key, value, tags, author_agent_id, channel_context, created_at \
                       FROM shared_knowledge \
                       ORDER BY rowid DESC LIMIT 100";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], knowledge_from_row)?;
            rows.filter_map(|r| r.ok()).collect::<Vec<_>>()
        };

        // Apply tag filter in Rust (simpler than fragile SQL LIKE on space-separated tokens).
        let filtered: Vec<KnowledgeEntry> = if let Some(tag_filter) = tags.filter(|s| !s.is_empty())
        {
            let required_tags: Vec<&str> = tag_filter.split_whitespace().collect();
            entries
                .into_iter()
                .filter(|e| {
                    let stored: Vec<&str> = e.tags.split_whitespace().collect();
                    required_tags.iter().all(|rt| stored.contains(rt))
                })
                .take(20)
                .collect()
        } else {
            entries.into_iter().take(20).collect()
        };

        Ok(filtered)
    }
}

fn knowledge_from_row(row: &rusqlite::Row) -> rusqlite::Result<KnowledgeEntry> {
    Ok(KnowledgeEntry {
        id: row.get(0)?,
        key: row.get(1)?,
        value: row.get(2)?,
        tags: row.get(3)?,
        author_agent_id: row.get(4)?,
        channel_context: row.get(5)?,
        created_at: parse_datetime(&row.get::<_, String>(6)?).to_rfc3339(),
    })
}
