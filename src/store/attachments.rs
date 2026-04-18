use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

/// Binary upload metadata persisted in SQLite and on disk under `attachments/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Random UUID primary key referenced by messages.
    pub id: String,
    /// Original client filename.
    pub filename: String,
    /// MIME type reported at upload.
    pub mime_type: String,
    /// Byte length on disk.
    pub size_bytes: i64,
    /// Path relative to the server data dir where the file is stored.
    pub stored_path: String,
    /// When the row was created.
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

impl Store {
    pub fn create_attachment(
        &self,
        filename: &str,
        mime_type: &str,
        size: i64,
        stored_path: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO attachments (id, filename, mime_type, size_bytes, stored_path) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, filename, mime_type, size, stored_path],
        )?;
        Ok(id)
    }

    pub fn get_attachment(&self, id: &str) -> Result<Option<Attachment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, filename, mime_type, size_bytes, stored_path, uploaded_at FROM attachments WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Attachment {
                id: row.get(0)?,
                filename: row.get(1)?,
                mime_type: row.get(2)?,
                size_bytes: row.get(3)?,
                stored_path: row.get(4)?,
                uploaded_at: parse_datetime(&row.get::<_, String>(5)?),
            })
        })?;
        Ok(rows.next().transpose()?)
    }
}
