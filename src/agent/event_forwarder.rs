//! Translates `DriverEvent`s from a [`RuntimeDriver`] into the three
//! observable state surfaces the rest of Chorus consumes:
//!
//!   1. `activity_logs` — per-agent activity entries the frontend polls.
//!   2. `trace_store` + broadcast `trace_tx` — Telescope trace events.
//!   3. `store.agent_sessions` — persisted session ID for resume.
//!
//! Runs as a detached `tokio::spawn` per agent for the lifetime of the
//! agent's `AgentHandle`. The task exits when the driver drops its side of
//! the `mpsc::Sender<DriverEvent>` (e.g. on `stop_agent`). The returned
//! `JoinHandle` is stored in [`super::manager::V2Agent::_event_tasks`] so
//! it's dropped when the agent is removed from the manager's map.
//!
//! This module deliberately owns no state: every input is passed in as
//! owned `String` / `Arc` / channel handle, which keeps the forwarder
//! testable in isolation (feed it a scripted `Receiver` and assert the
//! writes). The one exception is the `Completed` branch, which briefly
//! locks the manager's agents map to deliver a deferred notification when
//! messages arrived while the agent was mid-turn.
//!
//! Extracted from `manager.rs` because the fan-out is its own concept: the
//! lifecycle methods (`start_agent`, `stop_agent`, …) own the agents map
//! and driver registry; this module owns the event-to-state translation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, trace, warn};

use crate::agent::activity_log::{self, ActivityEntry, ActivityLogMap};
use crate::agent::drivers::v2::{AgentEventItem, AgentState, DriverEvent, PromptReq};
use crate::agent::manager::V2Agent;
use crate::agent::trace::{self, AgentTraceStore, TraceEvent, TraceEventKind};
use crate::store::Store;

/// Extract a short human-readable summary from an ACP tool-call `input`
/// object. Probes the common argument keys drivers use (file paths,
/// commands, queries) and returns the first match. Empty string when the
/// input isn't an object or none of the keys are present — the caller is
/// expected to treat empty as "no preview available."
fn summarize_input(input: &serde_json::Value) -> String {
    if !input.is_object() {
        return String::new();
    }
    let obj = input.as_object().unwrap();
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
/// or a ToolCall) or when the turn completes. Never called mid-stream —
/// the forwarder accumulates Thinking items into `pending_thinking` and
/// only flushes once the concept is "done."
fn flush_thinking(
    text: &str,
    agent_name: &str,
    trace_store: &AgentTraceStore,
    trace_tx: &broadcast::Sender<TraceEvent>,
    activity_logs: &ActivityLogMap,
) {
    let preview: String = text.chars().take(200).collect();
    let preview = if text.chars().count() > 200 {
        format!("{preview}\u{2026}")
    } else {
        preview
    };
    trace!(agent = %agent_name, thought = %preview, "v2: thinking block complete");
    activity_log::push_activity(
        activity_logs,
        agent_name,
        ActivityEntry::Thinking {
            text: text.to_string(),
        },
    );
    let (run_id, _) = trace_store.ensure_run(agent_name);
    let seq = trace_store.next_seq(agent_name);
    let ch = trace_store.run_channel_id(agent_name);
    let _ = trace_tx.send(trace::build_trace_event(
        run_id,
        agent_name,
        ch,
        seq,
        TraceEventKind::Thinking {
            text: text.to_string(),
        },
    ));
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
    let (run_id, _) = trace_store.ensure_run(agent_name);
    let seq = trace_store.next_seq(agent_name);
    let ch = trace_store.run_channel_id(agent_name);
    let _ = trace_tx.send(trace::build_trace_event(
        run_id,
        agent_name,
        ch,
        seq,
        TraceEventKind::Text {
            text: text.to_string(),
        },
    ));
}

/// Spawn the per-agent event-forwarder task.
///
/// Takes owned / `Arc` handles to every state surface it writes to, so it
/// has no shared mutable access with the caller beyond the agents-map
/// lock it acquires briefly in the `Completed` arm. Returns the `JoinHandle`
/// of the spawned task — store it on the agent's `V2Agent` so it's dropped
/// when the agent is removed.
pub(super) fn spawn_v2_event_forwarder(
    _agent_name: String,
    mut event_rx: tokio::sync::mpsc::Receiver<DriverEvent>,
    activity_logs: Arc<ActivityLogMap>,
    trace_store: Arc<AgentTraceStore>,
    trace_tx: broadcast::Sender<TraceEvent>,
    store: Arc<Store>,
    agents: Arc<Mutex<HashMap<String, V2Agent>>>,
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
                    info!(agent = %key, session = %session_id, "v2: session attached");
                    let _ = store.update_agent_session(key, Some(session_id));
                    activity_log::set_activity_state(&activity_logs, key, "online", "Ready");
                }

                DriverEvent::Lifecycle { ref key, ref state } => match state {
                    AgentState::Starting => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            "working",
                            "Starting\u{2026}",
                        );
                    }
                    AgentState::Active { .. } => {
                        activity_log::set_activity_state(&activity_logs, key, "online", "Idle");
                    }
                    AgentState::Closed => {
                        activity_log::set_activity_state(&activity_logs, key, "offline", "Stopped");
                    }
                    _ => {}
                },

                DriverEvent::Output {
                    ref key,
                    run_id: _,
                    ref item,
                } => {
                    match item {
                        AgentEventItem::Thinking { text } => {
                            pending_thinking.push_str(text);
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                "thinking",
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
                            info!(agent = %key, tool = %name, "v2: tool call");
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
                            activity_log::set_activity_state(&activity_logs, key, "working", name);
                            let (rid, _) = trace_store.ensure_run(key);
                            let seq = trace_store.next_seq(key);
                            let ch = trace_store.run_channel_id(key);
                            let _ = trace_tx.send(trace::build_trace_event(
                                rid,
                                key,
                                ch,
                                seq,
                                TraceEventKind::ToolCall {
                                    tool_name: name.clone(),
                                    tool_input,
                                },
                            ));
                        }
                        AgentEventItem::ToolResult { content } => {
                            let tool_name = last_tool_raw_name.clone().unwrap_or_default();
                            activity_log::upsert_tool_result_activity(
                                &activity_logs,
                                key,
                                tool_name.clone(),
                                content.clone(),
                            );
                            let (rid, _) = trace_store.ensure_run(key);
                            let seq = trace_store.next_seq(key);
                            let ch = trace_store.run_channel_id(key);
                            let _ = trace_tx.send(trace::build_trace_event(
                                rid,
                                key,
                                ch,
                                seq,
                                TraceEventKind::ToolResult {
                                    tool_name,
                                    content: content.clone(),
                                },
                            ));
                        }
                        AgentEventItem::TurnEnd => {
                            if let Some(run_id) = trace_store.active_run_id(key) {
                                let seq = trace_store.next_seq(key);
                                let ch = trace_store.run_channel_id(key);
                                let _ = trace_tx.send(trace::build_trace_event(
                                    run_id,
                                    key,
                                    ch,
                                    seq,
                                    TraceEventKind::TurnEnd,
                                ));
                                trace_store.end_run(key);
                            }
                            activity_log::set_activity_state(&activity_logs, key, "online", "Idle");
                        }
                        // Thinking/Text handled above via continue.
                        _ => {}
                    }
                }

                DriverEvent::Completed {
                    ref key,
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
                    info!(agent = %key, reason = ?result.finish_reason, "v2: run completed");
                    if !result.session_id.is_empty() {
                        let _ = store.update_agent_session(key, Some(&result.session_id));
                    }
                    if let Some(rid) = trace_store.active_run_id(key) {
                        let seq = trace_store.next_seq(key);
                        let ch = trace_store.run_channel_id(key);
                        let _ = trace_tx.send(trace::build_trace_event(
                            rid,
                            key,
                            ch,
                            seq,
                            TraceEventKind::TurnEnd,
                        ));
                        trace_store.end_run(key);
                    }
                    activity_log::set_activity_state(&activity_logs, key, "online", "Idle");

                    // If messages arrived while we were busy (init turn or any turn),
                    // deliver the notification immediately now that the turn is done.
                    {
                        let mut agents_guard = agents.lock().await;
                        if let Some(agent) = agents_guard.get_mut(key) {
                            let count = agent.pending_notification_count;
                            if count > 0 {
                                agent.pending_notification_count = 0;
                                let plural = if count > 1 { "s" } else { "" };
                                let them = if count > 1 { "them" } else { "it" };
                                let notification = format!(
                                    "[System notification: You have {count} new message{plural} \
                                     waiting. Call check_messages to read {them} when you're ready.]"
                                );
                                let (run_id, _) = trace_store.ensure_run(key);
                                let seq = trace_store.next_seq(key);
                                let ch = trace_store.run_channel_id(key);
                                let _ = trace_tx.send(trace::build_trace_event(
                                    run_id,
                                    key,
                                    ch,
                                    seq,
                                    TraceEventKind::Reading,
                                ));
                                info!(agent = %key, count = count, "delivering deferred notification after turn completion");
                                if let Err(e) = agent
                                    .handle
                                    .prompt(PromptReq {
                                        text: notification,
                                        attachments: vec![],
                                    })
                                    .await
                                {
                                    warn!(agent = %key, error = %e, "failed to deliver deferred notification");
                                }
                            }
                        }
                    }
                }

                DriverEvent::Failed {
                    ref key,
                    run_id: _,
                    ref error,
                } => {
                    let msg = format!("{error:?}");
                    error!(agent = %key, error = %msg, "v2: run failed");
                    if let Some(rid) = trace_store.active_run_id(key) {
                        let seq = trace_store.next_seq(key);
                        let ch = trace_store.run_channel_id(key);
                        let _ = trace_tx.send(trace::build_trace_event(
                            rid,
                            key,
                            ch,
                            seq,
                            TraceEventKind::Error {
                                message: msg.clone(),
                            },
                        ));
                        trace_store.end_run(key);
                    }
                    activity_log::set_activity_state(&activity_logs, key, "error", &msg);
                }
            }
        }
    })
}
