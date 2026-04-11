use std::collections::HashMap;
use std::time::Duration;

use rusqlite::{params, Connection};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::agent::trace::{TraceEvent, TraceEventKind};

/// Batch-writes trace events to SQLite and computes trace_summary on run end.
///
/// Uses a separate SQLite connection to avoid blocking the main Store mutex.
/// Batches events by 50 OR 100ms window, whichever comes first.
/// On TurnEnd/Error, flushes immediately and updates the message's trace_summary.
pub fn spawn_trace_writer(db_path: String, mut trace_rx: broadcast::Receiver<TraceEvent>) {
    tokio::spawn(async move {
        // Open a separate connection for trace writes.
        let conn = match Connection::open(&db_path) {
            Ok(c) => {
                let _ = c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
                c
            }
            Err(e) => {
                warn!(error = %e, "trace_writer: failed to open db, trace persistence disabled");
                return;
            }
        };

        let mut batch: Vec<TraceEvent> = Vec::with_capacity(64);
        // Per-run accumulator for summary computation.
        let mut run_events: HashMap<String, Vec<TraceEvent>> = HashMap::new();

        loop {
            // Wait for first event or exit.
            let event = tokio::select! {
                result = trace_rx.recv() => {
                    match result {
                        Ok(e) => e,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(dropped = n, "trace_writer: lagged, events dropped");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            };

            let is_run_end = matches!(
                event.kind,
                TraceEventKind::TurnEnd | TraceEventKind::Error { .. }
            );
            run_events
                .entry(event.run_id.clone())
                .or_default()
                .push(event.clone());
            batch.push(event);

            if is_run_end {
                // Flush immediately on run end.
                flush_batch(&conn, &mut batch);
                // Compute and write trace_summary for the finished run.
                for (run_id, events) in run_events.drain() {
                    if events.iter().any(|e| {
                        matches!(
                            e.kind,
                            TraceEventKind::TurnEnd | TraceEventKind::Error { .. }
                        )
                    }) {
                        write_trace_summary(&conn, &run_id, &events);
                    }
                }
                continue;
            }

            // Collect more events within 100ms window, up to 50.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
            while batch.len() < 50 {
                match tokio::time::timeout_at(deadline, trace_rx.recv()).await {
                    Ok(Ok(event)) => {
                        let is_end = matches!(
                            event.kind,
                            TraceEventKind::TurnEnd | TraceEventKind::Error { .. }
                        );
                        run_events
                            .entry(event.run_id.clone())
                            .or_default()
                            .push(event.clone());
                        batch.push(event);
                        if is_end {
                            break;
                        }
                    }
                    Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                        warn!(dropped = n, "trace_writer: lagged during batch");
                        break;
                    }
                    Ok(Err(broadcast::error::RecvError::Closed)) => {
                        flush_batch(&conn, &mut batch);
                        return;
                    }
                    Err(_timeout) => break,
                }
            }

            flush_batch(&conn, &mut batch);

            // Write summaries for any completed runs.
            let completed_runs: Vec<String> = run_events
                .iter()
                .filter(|(_, events)| {
                    events.iter().any(|e| {
                        matches!(
                            e.kind,
                            TraceEventKind::TurnEnd | TraceEventKind::Error { .. }
                        )
                    })
                })
                .map(|(run_id, _)| run_id.clone())
                .collect();
            for run_id in completed_runs {
                if let Some(events) = run_events.remove(&run_id) {
                    write_trace_summary(&conn, &run_id, &events);
                }
            }
        }

        // Final flush on shutdown.
        if !batch.is_empty() {
            flush_batch(&conn, &mut batch);
        }
        info!("trace_writer: shutdown");
    });
}

fn flush_batch(conn: &Connection, batch: &mut Vec<TraceEvent>) {
    if batch.is_empty() {
        return;
    }
    let count = batch.len();
    if let Err(e) = write_events(conn, batch) {
        warn!(error = %e, count, "trace_writer: failed to write batch");
    } else {
        debug!(count, "trace_writer: flushed batch");
    }
    batch.clear();
}

fn write_events(conn: &Connection, events: &[TraceEvent]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO trace_events (run_id, seq, timestamp_ms, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for event in events {
            let kind_str = event_kind_str(&event.kind);
            let data_json = serde_json::to_string(&event.kind).unwrap_or_default();
            stmt.execute(params![
                event.run_id,
                event.seq,
                event.timestamp_ms,
                kind_str,
                data_json,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn event_kind_str(kind: &TraceEventKind) -> &'static str {
    match kind {
        TraceEventKind::Reading => "reading",
        TraceEventKind::Thinking { .. } => "thinking",
        TraceEventKind::ToolCall { .. } => "tool_call",
        TraceEventKind::ToolResult { .. } => "tool_result",
        TraceEventKind::Text { .. } => "text",
        TraceEventKind::TurnEnd => "turn_end",
        TraceEventKind::Error { .. } => "error",
    }
}

fn write_trace_summary(conn: &Connection, run_id: &str, events: &[TraceEvent]) {
    let summary = compute_trace_summary(events);
    let summary_json = match serde_json::to_string(&summary) {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, run_id, "trace_writer: failed to serialize summary");
            return;
        }
    };

    match conn.execute(
        "UPDATE messages SET trace_summary = ?1 WHERE run_id = ?2",
        params![summary_json, run_id],
    ) {
        Ok(0) => {
            debug!(
                run_id,
                "trace_writer: no message found for run_id (agent may not have replied yet)"
            );
        }
        Ok(_) => {
            debug!(run_id, "trace_writer: wrote trace_summary");
        }
        Err(e) => {
            warn!(error = %e, run_id, "trace_writer: failed to update trace_summary");
        }
    }
}

#[derive(serde::Serialize)]
struct TraceSummary {
    #[serde(rename = "toolCalls")]
    tool_calls: u32,
    duration: u64,
    status: &'static str,
    categories: HashMap<String, u32>,
}

fn compute_trace_summary(events: &[TraceEvent]) -> TraceSummary {
    let mut tool_calls: u32 = 0;
    let mut categories: HashMap<String, u32> = HashMap::new();
    let mut has_error = false;

    let first_ts = events.first().map(|e| e.timestamp_ms).unwrap_or(0);
    let last_ts = events.last().map(|e| e.timestamp_ms).unwrap_or(0);

    for event in events {
        match &event.kind {
            TraceEventKind::ToolCall { tool_name, .. } => {
                tool_calls += 1;
                let category = classify_tool(tool_name);
                *categories.entry(category.to_string()).or_insert(0) += 1;
            }
            TraceEventKind::Error { .. } => {
                has_error = true;
            }
            _ => {}
        }
    }

    TraceSummary {
        tool_calls,
        duration: last_ts.saturating_sub(first_ts),
        status: if has_error { "error" } else { "completed" },
        categories,
    }
}

fn classify_tool(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("read_file")
        || lower.contains("write_file")
        || lower.contains("edit")
        || lower.contains("list_dir")
    {
        "file"
    } else if lower.contains("search") || lower.contains("grep") || lower.contains("find") {
        "search"
    } else if lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("exec")
        || lower.contains("command")
    {
        "terminal"
    } else if lower.contains("http")
        || lower.contains("fetch")
        || lower.contains("curl")
        || lower.contains("web")
    {
        "net"
    } else {
        "other"
    }
}
