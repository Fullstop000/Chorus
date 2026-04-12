pub mod error;

use std::path::{Path, PathBuf};

/// Parse a datetime string in `%Y-%m-%d %H:%M:%S` format, falling back to now.
pub fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc())
        .unwrap_or_else(|_| chrono::Utc::now())
}

/// Derive the server data directory from the SQLite path.
pub fn derive_data_dir(path: &str) -> PathBuf {
    if path == ":memory:" {
        return std::env::temp_dir().join(format!("chorus-memory-{}", uuid::Uuid::new_v4()));
    }

    Path::new(path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Sanitize a raw FTS5 query string so agent-supplied input cannot inject FTS5 syntax
/// that causes a parse error. We escape double-quotes and strip bare operators.
pub fn sanitize_fts_query(raw: &str) -> String {
    // Wrap each word in double-quotes so FTS5 treats them as phrase literals.
    // This prevents injection of FTS5 operators like AND/OR/NOT/NEAR.
    raw.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}
