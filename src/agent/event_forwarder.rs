//! Translates `DriverEvent`s from a [`RuntimeDriver`] into the three
//! observable state surfaces the rest of Chorus consumes:
//!
//!   1. `activity_logs` — per-agent activity entries the frontend polls.
//!   2. `trace_store` + broadcast `trace_tx` — Telescope trace events.
//!   3. `store.agent_sessions` — persisted session ID for resume.
//!
//! Runs as a detached `tokio::spawn` per agent for the lifetime of the
//! agent's `AgentSessionHandle`. The task exits when the driver drops its side of
//! the `mpsc::Sender<DriverEvent>` (e.g. on `stop_agent`). The returned
//! `JoinHandle` is stored on the agent's `ManagedAgent` so it's dropped
//! when the agent is removed from the manager's map.
//!
//! Every input is passed in as owned `String` / `Arc` / channel handle,
//! so the forwarder is testable in isolation (feed it a scripted
//! `Receiver` and assert the writes). The one exception is the
//! `Completed` branch, which briefly locks the manager's agents map to
//! deliver a deferred notification when messages arrived mid-turn.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, trace, warn};

use crate::agent::activity_log::{
    self, ActivityEntry, ActivityLogMap, ACTIVITY_ERROR, ACTIVITY_OFFLINE, ACTIVITY_ONLINE,
    ACTIVITY_THINKING, ACTIVITY_WORKING,
};
use crate::agent::drivers::{AgentEventItem, AgentState, DriverEvent};
use crate::agent::manager::ManagedAgent;
use crate::agent::trace::{self, AgentTraceStore, TraceEvent, TraceEventKind};
use crate::store::Store;

/// Extract a short human-readable summary from an ACP tool-call `input`
/// object. Probes the common argument keys drivers use (`file_path`,
/// `path`, `command`, `query`, `url`) and returns the first string match.
/// Returns empty string when none of the keys are present — callers treat
/// empty as "no preview available."
fn summarize_input(input: &serde_json::Value) -> String {
    let Some(obj) = input.as_object() else {
        return String::new();
    };
    for key in &["file_path", "path", "command", "query", "url"] {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Emit a buffered run of thinking text as a single trace event + activity
/// entry. Called when the driver switches away from Thinking (e.g. to Text
/// or a ToolCall) or when the turn completes.
fn flush_thinking(
    text: &str,
    agent_name: &str,
    trace_store: &AgentTraceStore,
    trace_tx: &broadcast::Sender<TraceEvent>,
    activity_logs: &ActivityLogMap,
) {
    // Single-pass truncation: take up to 200 chars, then check whether
    // more remain rather than re-walking `text` with `chars().count()`.
    let mut iter = text.chars();
    let preview: String = iter.by_ref().take(200).collect();
    let preview = if iter.next().is_some() {
        format!("{preview}\u{2026}")
    } else {
        preview
    };
    trace!(agent = %agent_name, thought = %preview, "thinking block complete");
    activity_log::push_activity(
        activity_logs,
        agent_name,
        ActivityEntry::Thinking {
            text: text.to_string(),
        },
    );
    trace::emit_event(
        trace_store,
        trace_tx,
        agent_name,
        TraceEventKind::Thinking {
            text: text.to_string(),
        },
    );
}

/// Emit a buffered run of plain text as a single trace event. Unlike
/// `flush_thinking`, text is already pushed to `activity_logs` as it
/// arrives (so the frontend can stream it); this only flushes the
/// telescope trace side.
fn flush_text(
    text: &str,
    agent_name: &str,
    trace_store: &AgentTraceStore,
    trace_tx: &broadcast::Sender<TraceEvent>,
) {
    trace::emit_event(
        trace_store,
        trace_tx,
        agent_name,
        TraceEventKind::Text {
            text: text.to_string(),
        },
    );
}

/// Spawn the per-agent event-forwarder task. Returns the `JoinHandle` of
/// the spawned task — store it on the agent's `ManagedAgent` so it's
/// dropped (and the task aborted) when the agent is removed.
pub(super) fn spawn_event_forwarder(
    mut event_rx: tokio::sync::mpsc::Receiver<DriverEvent>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
    trace_tx: broadcast::Sender<TraceEvent>,
    store: Arc<Store>,
    agents: Arc<Mutex<HashMap<String, ManagedAgent>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut pending_thinking = String::new();
        let mut pending_text = String::new();
        let mut last_tool_raw_name: Option<String> = None;

        while let Some(event) = event_rx.recv().await {
            match event {
                DriverEvent::SessionAttached {
                    ref key,
                    ref session_id,
                } => {
                    info!(agent = %key, session = %session_id, "session attached");
                    let _ = store.update_agent_session(key, Some(session_id));
                    activity_log::set_activity_state(&activity_logs, key, ACTIVITY_ONLINE, "Ready");
                }

                DriverEvent::Lifecycle { ref key, ref state } => match state {
                    AgentState::Starting => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_WORKING,
                            "Starting\u{2026}",
                        );
                    }
                    AgentState::Active { .. } => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_ONLINE,
                            "Idle",
                        );
                    }
                    AgentState::Closed => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_OFFLINE,
                            "Stopped",
                        );
                    }
                    _ => {}
                },

                DriverEvent::Output {
                    ref key,
                    session_id: _,
                    run_id: _,
                    ref item,
                } => {
                    match item {
                        AgentEventItem::Thinking { text } => {
                            pending_thinking.push_str(text);
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                ACTIVITY_THINKING,
                                "Thinking\u{2026}",
                            );
                            continue;
                        }
                        AgentEventItem::Text { text } => {
                            if !pending_thinking.is_empty() {
                                flush_thinking(
                                    &pending_thinking,
                                    key,
                                    &trace_store,
                                    &trace_tx,
                                    &activity_logs,
                                );
                                pending_thinking.clear();
                            }
                            activity_log::push_activity(
                                &activity_logs,
                                key,
                                ActivityEntry::Text { text: text.clone() },
                            );
                            pending_text.push_str(text);
                            continue;
                        }
                        _ => {
                            if !pending_thinking.is_empty() {
                                flush_thinking(
                                    &pending_thinking,
                                    key,
                                    &trace_store,
                                    &trace_tx,
                                    &activity_logs,
                                );
                                pending_thinking.clear();
                            }
                            if !pending_text.is_empty() {
                                flush_text(&pending_text, key, &trace_store, &trace_tx);
                                pending_text.clear();
                            }
                        }
                    }

                    match item {
                        AgentEventItem::ToolCall { name, input } => {
                            info!(agent = %key, tool = %name, "tool call");
                            last_tool_raw_name = Some(name.clone());
                            let tool_input = summarize_input(input);
                            activity_log::push_activity(
                                &activity_logs,
                                key,
                                ActivityEntry::ToolCall {
                                    tool_name: name.clone(),
                                    tool_input: tool_input.clone(),
                                },
                            );
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                ACTIVITY_WORKING,
                                name,
                            );
                            trace::emit_event(
                                &trace_store,
                                &trace_tx,
                                key,
                                TraceEventKind::ToolCall {
                                    tool_name: name.clone(),
                                    tool_input,
                                },
                            );
                        }
                        AgentEventItem::ToolResult { content } => {
                            let tool_name = last_tool_raw_name.clone().unwrap_or_default();
                            activity_log::upsert_tool_result_activity(
                                &activity_logs,
                                key,
                                tool_name.clone(),
                                content.clone(),
                            );
                            trace::emit_event(
                                &trace_store,
                                &trace_tx,
                                key,
                                TraceEventKind::ToolResult {
                                    tool_name,
                                    content: content.clone(),
                                },
                            );
                        }
                        AgentEventItem::TurnEnd => {
                            trace::emit_active_event(
                                &trace_store,
                                &trace_tx,
                                key,
                                TraceEventKind::TurnEnd,
                            );
                            trace_store.end_run(key);
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                ACTIVITY_ONLINE,
                                "Idle",
                            );
                        }
                        // Thinking / Text handled above via `continue`.
                        _ => {}
                    }
                }

                DriverEvent::Completed {
                    ref key,
                    ref session_id,
                    run_id: _,
                    ref result,
                } => {
                    if !pending_thinking.is_empty() {
                        flush_thinking(
                            &pending_thinking,
                            key,
                            &trace_store,
                            &trace_tx,
                            &activity_logs,
                        );
                        pending_thinking.clear();
                    }
                    if !pending_text.is_empty() {
                        flush_text(&pending_text, key, &trace_store, &trace_tx);
                        pending_text.clear();
                    }
                    info!(agent = %key, reason = ?result.finish_reason, "run completed");
                    if !session_id.is_empty() {
                        let _ = store.update_agent_session(key, Some(session_id));
                    }
                    trace::emit_active_event(&trace_store, &trace_tx, key, TraceEventKind::TurnEnd);
                    trace_store.end_run(key);
                    activity_log::set_activity_state(&activity_logs, key, ACTIVITY_ONLINE, "Idle");

                    // Deliver any notifications that queued up while the
                    // agent was mid-turn. The debounce path in
                    // `notify_agent` uses the same method, so the
                    // Reading-trace + prompt format stays in one place.
                    let mut guard = agents.lock().await;
                    if let Some(agent) = guard.get_mut(key) {
                        match agent
                            .deliver_pending_notification(&trace_store, &trace_tx, key)
                            .await
                        {
                            Ok(count) if count > 0 => {
                                info!(agent = %key, count, "delivered deferred notification");
                            }
                            Ok(_) => {} // nothing pending
                            Err(e) => {
                                warn!(agent = %key, error = %e, "failed to deliver deferred notification");
                            }
                        }
                    }
                }

                DriverEvent::Failed {
                    ref key,
                    session_id: _,
                    run_id: _,
                    ref error,
                } => {
                    let msg = format!("{error:?}");
                    error!(agent = %key, error = %msg, "run failed");
                    trace::emit_active_event(
                        &trace_store,
                        &trace_tx,
                        key,
                        TraceEventKind::Error {
                            message: msg.clone(),
                        },
                    );
                    trace_store.end_run(key);
                    activity_log::set_activity_state(&activity_logs, key, ACTIVITY_ERROR, &msg);
                }
            }
        }
    })
}
