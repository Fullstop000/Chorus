//! Translates `DriverEvent`s from a [`RuntimeDriver`] into the three
//! observable state surfaces the rest of Chorus consumes:
//!
//!   1. `activity_logs` — per-agent activity entries the frontend polls.
//!   2. `trace_store` + broadcast `trace_tx` — Telescope trace events.
//!   3. `store.agent_sessions` — persisted session ID for resume.
//!
//! Runs as a detached `tokio::spawn` per agent for the lifetime of the
//! agent's `Session`. The task exits when the driver drops its side of
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
use crate::agent::drivers::{AgentEventItem, DriverEvent, ProcessState};
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

/// Best-effort session persistence with structured logging on failure.
/// Resume continuity depends on this row; if it silently disappears the
/// next start_agent issues `SessionIntent::New` and the user loses history.
/// `site` is a short tag ("attach" / "completed") distinguishing the caller.
fn persist_session(store: &Store, agent_name: &str, session_id: &str, site: &str) {
    match store.get_agent(agent_name) {
        Ok(Some(agent)) => {
            if let Err(err) = store.record_session(&agent.id, session_id, &agent.runtime) {
                warn!(
                    agent = %agent_name,
                    session = %session_id,
                    site,
                    err = %err,
                    "failed to persist session"
                );
            }
        }
        Ok(None) => {
            warn!(
                agent = %agent_name,
                session = %session_id,
                site,
                "agent row missing while persisting session"
            );
        }
        Err(err) => {
            warn!(
                agent = %agent_name,
                session = %session_id,
                site,
                err = %err,
                "failed to load agent while persisting session"
            );
        }
    }
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
        // Pending buffers are keyed by `session_id` because Stage 2 drivers can
        // multiplex multiple concurrent sessions onto one forwarder (Phase 0.9
        // Stage 2). Before this change the buffers were per-forwarder scalars,
        // which cross-contaminated: session A's Thinking text could flush when
        // session B emitted a Text event, or a ToolResult from B could be
        // attributed to A's most recent ToolCall. Keying by `session_id`
        // isolates each session's in-flight stream; entries are removed in the
        // `Completed`/`Failed` branches so the maps don't leak across runs.
        //
        // `trace_store` and `activity_logs` remain per-agent for Stage 2: the
        // AgentManager still exposes one-handle-per-agent and concurrent
        // multi-session events are uncommon in production. `AgentTraceStore`
        // treats a duplicate `end_run` as a no-op (see `AgentRunState::end_run`)
        // so the two possible end-of-turn paths (`Output { TurnEnd }` and
        // `Completed`) firing back-to-back under multi-session is harmless.
        // Promoting trace/activity storage to per-session is a Phase 3 item.
        let mut pending_thinking: HashMap<String, String> = HashMap::new();
        let mut pending_text: HashMap<String, String> = HashMap::new();
        let mut last_tool_raw_name: HashMap<String, String> = HashMap::new();
        // Tracks "did this session produce any user-visible output during its
        // current run?" — counts Text and ToolCall events. On Completed with
        // zero events, we surface ACTIVITY_ERROR with a "no response" detail
        // so the user sees an explicit signal instead of a silent no-op
        // (e.g. when codex auth dies, the run still reaches `FinishReason::Natural`
        // but the agent produced nothing). Cleared in Completed/Failed.
        let mut output_count: HashMap<String, u32> = HashMap::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                DriverEvent::SessionAttached {
                    ref key,
                    ref session_id,
                } => {
                    info!(agent = %key, session = %session_id, "session attached");
                    persist_session(&store, key, session_id, "attach");
                    activity_log::set_activity_state(&activity_logs, key, ACTIVITY_ONLINE, "Ready");
                }

                DriverEvent::Lifecycle { ref key, ref state } => match state {
                    ProcessState::Starting => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_WORKING,
                            "Starting\u{2026}",
                        );
                    }
                    ProcessState::Active { .. } => {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_ONLINE,
                            "Idle",
                        );
                    }
                    ProcessState::Closed => {
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
                    ref session_id,
                    run_id: _,
                    ref item,
                } => {
                    match item {
                        AgentEventItem::Thinking { text } => {
                            pending_thinking
                                .entry(session_id.clone())
                                .or_default()
                                .push_str(text);
                            activity_log::set_activity_state(
                                &activity_logs,
                                key,
                                ACTIVITY_THINKING,
                                "Thinking\u{2026}",
                            );
                            continue;
                        }
                        AgentEventItem::Text { text } => {
                            if let Some(buf) = pending_thinking.get_mut(session_id) {
                                if !buf.is_empty() {
                                    flush_thinking(
                                        buf,
                                        key,
                                        &trace_store,
                                        &trace_tx,
                                        &activity_logs,
                                    );
                                    buf.clear();
                                }
                            }
                            activity_log::push_activity(
                                &activity_logs,
                                key,
                                ActivityEntry::Text { text: text.clone() },
                            );
                            pending_text
                                .entry(session_id.clone())
                                .or_default()
                                .push_str(text);
                            *output_count.entry(session_id.clone()).or_default() += 1;
                            continue;
                        }
                        _ => {
                            if let Some(buf) = pending_thinking.get_mut(session_id) {
                                if !buf.is_empty() {
                                    flush_thinking(
                                        buf,
                                        key,
                                        &trace_store,
                                        &trace_tx,
                                        &activity_logs,
                                    );
                                    buf.clear();
                                }
                            }
                            if let Some(buf) = pending_text.get_mut(session_id) {
                                if !buf.is_empty() {
                                    flush_text(buf, key, &trace_store, &trace_tx);
                                    buf.clear();
                                }
                            }
                        }
                    }

                    match item {
                        AgentEventItem::ToolCall { name, input } => {
                            info!(agent = %key, tool = %name, "tool call");
                            last_tool_raw_name.insert(session_id.clone(), name.clone());
                            *output_count.entry(session_id.clone()).or_default() += 1;
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
                            let tool_name = last_tool_raw_name
                                .get(session_id)
                                .cloned()
                                .unwrap_or_default();
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
                            // `end_run` is keyed by agent (see trace::AgentRunState::end_run)
                            // and is idempotent — safe to call from both this branch and
                            // the sibling `Completed` branch, even under multi-session.
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
                    if let Some(buf) = pending_thinking.remove(session_id) {
                        if !buf.is_empty() {
                            flush_thinking(&buf, key, &trace_store, &trace_tx, &activity_logs);
                        }
                    }
                    if let Some(buf) = pending_text.remove(session_id) {
                        if !buf.is_empty() {
                            flush_text(&buf, key, &trace_store, &trace_tx);
                        }
                    }
                    // Drop this session's tool-name binding so it can't leak
                    // into a future run reusing the same session_id.
                    last_tool_raw_name.remove(session_id);
                    let produced = output_count.remove(session_id).unwrap_or(0);
                    info!(
                        agent = %key,
                        reason = ?result.finish_reason,
                        produced,
                        "run completed",
                    );
                    if !session_id.is_empty() {
                        persist_session(&store, key, session_id, "completed");
                    }
                    trace::emit_active_event(&trace_store, &trace_tx, key, TraceEventKind::TurnEnd);
                    trace_store.end_run(key);
                    if produced == 0 {
                        // Soft-failure surface: the run reached its natural end
                        // without ever calling a tool or emitting text. The user
                        // who triggered this run sees nothing in chat unless we
                        // mark the agent as errored — common cause is codex auth
                        // dying mid-run (the underlying CLI prints ERROR to stderr
                        // and then `respond` returns silently with reason=Natural).
                        let detail = format!(
                            "Finished without responding (reason: {:?})",
                            result.finish_reason
                        );
                        warn!(agent = %key, %detail, "silent run finish");
                        trace::emit_event(
                            &trace_store,
                            &trace_tx,
                            key,
                            TraceEventKind::Error {
                                message: detail.clone(),
                            },
                        );
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_ERROR,
                            &detail,
                        );
                    } else {
                        activity_log::set_activity_state(
                            &activity_logs,
                            key,
                            ACTIVITY_ONLINE,
                            "Idle",
                        );
                    }

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
                    ref session_id,
                    run_id: _,
                    ref error,
                } => {
                    // Clean up any pending buffers for the failing session so
                    // half-streamed thinking/text doesn't silently leak into a
                    // later run reusing the same session_id.
                    pending_thinking.remove(session_id);
                    pending_text.remove(session_id);
                    last_tool_raw_name.remove(session_id);
                    output_count.remove(session_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::drivers::{FinishReason, RunResult};
    use tokio::sync::mpsc;

    /// Collect every TraceEvent emitted on `rx` until the sender is dropped.
    async fn collect_traces(mut rx: broadcast::Receiver<TraceEvent>) -> Vec<TraceEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = rx.recv().await {
            out.push(evt);
        }
        out
    }

    /// Two concurrent sessions on the same agent each stream Thinking text and
    /// then hit TurnEnd. Before the per-session buffer fix, session A's
    /// Thinking text would be flushed under session B's TurnEnd (or vice
    /// versa), producing duplicated or cross-contaminated Thinking trace
    /// events. This test proves each TurnEnd now flushes only its own
    /// session's buffered text.
    #[tokio::test]
    async fn pending_buffers_are_isolated_per_session() {
        // Build the minimum wiring to exercise the forwarder. Store uses
        // in-memory SQLite; the agents map is empty (no ManagedAgent) because
        // the `Completed` branch's lookup is `get_mut(key)` which returns
        // None and short-circuits without touching any state under test.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("chorus.db");
        let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
        let activity_logs = Arc::new(ActivityLogMap::default());
        let trace_store = Arc::new(AgentTraceStore::new());
        let (trace_tx, trace_rx) = broadcast::channel::<TraceEvent>(64);
        let agents: Arc<Mutex<HashMap<String, ManagedAgent>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (event_tx, event_rx) = mpsc::channel::<DriverEvent>(64);
        let forwarder = spawn_event_forwarder(
            event_rx,
            activity_logs,
            trace_store,
            trace_tx.clone(),
            store,
            agents,
        );

        let key = "bot".to_string();
        let sid_a = "session-A".to_string();
        let sid_b = "session-B".to_string();

        // A: Thinking "a-thought"
        event_tx
            .send(DriverEvent::Output {
                key: key.clone(),
                session_id: sid_a.clone(),
                run_id: uuid::Uuid::new_v4(),
                item: AgentEventItem::Thinking {
                    text: "a-thought".to_string(),
                },
            })
            .await
            .unwrap();
        // B: Thinking "b-thought" (different text, concurrent with A)
        event_tx
            .send(DriverEvent::Output {
                key: key.clone(),
                session_id: sid_b.clone(),
                run_id: uuid::Uuid::new_v4(),
                item: AgentEventItem::Thinking {
                    text: "b-thought".to_string(),
                },
            })
            .await
            .unwrap();
        // A completes — its TurnEnd-equivalent is the Completed branch here,
        // which flushes A's pending Thinking. (The `Output { TurnEnd }` path
        // is the in-turn variant; Completed is the authoritative end.)
        event_tx
            .send(DriverEvent::Completed {
                key: key.clone(),
                session_id: sid_a.clone(),
                run_id: uuid::Uuid::new_v4(),
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            })
            .await
            .unwrap();
        // B completes — flush B's pending Thinking.
        event_tx
            .send(DriverEvent::Completed {
                key: key.clone(),
                session_id: sid_b.clone(),
                run_id: uuid::Uuid::new_v4(),
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            })
            .await
            .unwrap();

        // Close the channel so the forwarder exits and we can await it.
        drop(event_tx);
        forwarder.await.unwrap();
        // Drop the original sender after the forwarder so collect_traces
        // terminates when the last subscriber sender is gone.
        drop(trace_tx);

        let traces = collect_traces(trace_rx).await;
        let thinking: Vec<String> = traces
            .into_iter()
            .filter_map(|e| match e.kind {
                TraceEventKind::Thinking { text } => Some(text),
                _ => None,
            })
            .collect();

        assert_eq!(
            thinking.len(),
            2,
            "expected exactly two Thinking flushes (one per session), got {thinking:?}"
        );
        assert!(
            thinking.contains(&"a-thought".to_string()),
            "session A's Thinking text missing: {thinking:?}"
        );
        assert!(
            thinking.contains(&"b-thought".to_string()),
            "session B's Thinking text missing: {thinking:?}"
        );
        // The regression: before the fix, A's buffer and B's buffer were one
        // shared scalar, so one of the flushes would carry concatenated text
        // ("a-thoughtb-thought") and the other would be empty or absent.
        for t in &thinking {
            assert!(
                t == "a-thought" || t == "b-thought",
                "cross-contamination: flushed text {t:?} is not exactly one session's payload"
            );
        }
    }
}
