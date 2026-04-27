//! Shared reader loop + response routing.
//!
//! The reader loop consumes the runtime's stdout, splits frames into
//! responses (routed by JSON-RPC `id` through `SharedReaderState::pending`)
//! and notifications (routed by sessionId through the per-session
//! `tool_accumulator`). All the routing logic that used to be triplicated
//! across kimi.rs / gemini.rs / opencode.rs lives here.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::super::acp_protocol::{self, AcpParsed, AcpUpdateItem};
use super::super::{
    AgentError, AgentEventItem, AgentKey, DriverEvent, FinishReason, ProcessState, RunId,
    RunResult,
};

use super::state::{PendingRequest, SessionState, SharedReaderState};

/// Consume the runtime's stdout and drive the shared reader state + event
/// emission.
///
/// Splits into:
///  - manual id-lookup dispatch for JSON-RPC RESPONSES (so id>=3 isn't
///    misclassified by `acp_protocol::parse_line` as PromptResponse when
///    it's actually a `session/new` response on a multi-session driver),
///  - `parse_line` for notifications (`session/update`) and server
///    requests (`session/request_permission`).
pub(super) async fn reader_loop(
    driver: &'static str,
    key: AgentKey,
    event_tx: mpsc::Sender<DriverEvent>,
    shared: Arc<Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
    stdout: std::process::ChildStdout,
) {
    let async_stdout = match tokio::process::ChildStdout::from_std(stdout) {
        Ok(s) => s,
        Err(e) => {
            warn!(key = %key, driver, error = %e, "failed to convert stdout to async");
            return;
        }
    };
    let reader = BufReader::new(async_stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        trace!(driver, line = %line, "stdout");

        // Try to extract id + session id before leaning on parse_line. We
        // need id to route responses; we need sessionId (from params) to
        // route notifications to the right session.
        let raw: Option<Value> = serde_json::from_str(&line).ok();

        // 1) JSON-RPC responses (have `id` + (`result` | `error`)).
        if let Some(ref msg) = raw {
            let is_response = msg.get("id").is_some()
                && (msg.get("result").is_some() || msg.get("error").is_some());
            if is_response {
                handle_response(driver, &key, &event_tx, &shared, &stdin_tx, msg).await;
                continue;
            }
        }

        // 2) Everything else: lean on parse_line for notifications /
        // permission requests / errors.
        let parsed = acp_protocol::parse_line(&line);
        match parsed {
            AcpParsed::InitializeResponse
            | AcpParsed::SessionResponse { .. }
            | AcpParsed::PromptResponse { .. } => {
                // Already handled by handle_response above. If we got
                // here, parse_line happened to match an Unknown that
                // looked like a response but our raw check didn't catch
                // — log and ignore.
                debug!(driver, line = %line, "response slipped past raw check — ignoring");
            }
            AcpParsed::SessionUpdate { items } => {
                let session_id = raw
                    .as_ref()
                    .and_then(|m| m.get("params"))
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                handle_session_update(driver, &key, &event_tx, &shared, session_id, items);
            }
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                let option_id = acp_protocol::pick_best_option_id(&options);
                debug!(
                    driver,
                    ?tool_name, request_id, option_id, "auto-approving permission"
                );
                let response = acp_protocol::build_permission_response_raw(request_id, option_id);
                let _ = stdin_tx.try_send(response);
            }
            AcpParsed::Error { message } => {
                // Without an id we can't pick which session — surface as
                // a generic Failed on the first in-flight session we
                // find.
                warn!(driver, message = %message, "ACP error (unrouted)");
                let mut s = shared.lock().unwrap();
                let target = s
                    .sessions
                    .iter()
                    .find(|(_, st)| matches!(st.state, ProcessState::PromptInFlight { .. }))
                    .map(|(sid, st)| (sid.clone(), st.run_id));
                if let Some((sid, Some(run_id))) = target {
                    let slot = s.sessions.get_mut(&sid).unwrap();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: sid.clone(),
                    };
                    let _ = event_tx.try_send(DriverEvent::Failed {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        error: AgentError::RuntimeReported(message),
                    });
                }
            }
            AcpParsed::Unknown => {}
        }
    }

    // EOF — runtime exited. Emit TransportClosed for every in-flight run,
    // then close out the event stream. Skip the `Lifecycle { Closed }`
    // emit if a concurrent `close()` already fired it
    // (`closed_emitted` flag) — otherwise subscribers see two identical
    // Closed events.
    let (drained, already_closed) = {
        let s = shared.lock().unwrap();
        let drained: Vec<(String, RunId)> = s
            .sessions
            .iter()
            .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
            .collect();
        let already_closed = s.closed_emitted.load(Ordering::SeqCst);
        (drained, already_closed)
    };
    for (sid, run_id) in drained {
        let _ = event_tx.try_send(DriverEvent::Completed {
            key: key.clone(),
            session_id: sid,
            run_id,
            result: RunResult {
                finish_reason: FinishReason::TransportClosed,
            },
        });
    }
    if !already_closed {
        // Claim the slot first so a concurrent close() sees it already
        // emitted and skips (guards the other direction too).
        let shared_emitted = {
            let s = shared.lock().unwrap();
            s.closed_emitted.clone()
        };
        if !shared_emitted.swap(true, Ordering::SeqCst) {
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: ProcessState::Closed,
            });
        }
    }
    {
        let mut s = shared.lock().unwrap();
        for st in s.sessions.values_mut() {
            st.state = ProcessState::Closed;
        }
    }
}

/// Test-only re-export of `handle_response`. The shared tests in
/// `acp_native::tests` exercise the response routing directly; the
/// production reader_loop is the only other caller, so the function stays
/// private otherwise.
#[cfg(test)]
pub(super) async fn handle_response_for_test(
    driver: &'static str,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
    msg: &Value,
) {
    handle_response(driver, key, event_tx, shared, stdin_tx, msg).await;
}

async fn handle_response(
    driver: &'static str,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
    msg: &Value,
) {
    let id = match msg.get("id").and_then(|v| v.as_u64()) {
        Some(id) => id,
        None => return,
    };
    let error_msg: Option<String> = msg
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if msg.get("error").is_some() {
                Some("unknown ACP error".to_string())
            } else {
                None
            }
        });

    let pending = shared.lock().unwrap().pending.remove(&id);
    let Some(pending) = pending else {
        debug!(driver, id, "response for unknown id — ignoring");
        return;
    };

    match pending {
        PendingRequest::Init => {
            let mut s = shared.lock().unwrap();
            s.phase = acp_protocol::AcpPhase::Active;
            // Some agents (gemini) require an `initialized` notification
            // after the `initialize` response. Send it now if configured.
            if let Some(notif) = s.initialized_notification.take() {
                let _ = stdin_tx.try_send(notif);
            }
            debug!(driver, "initialize response received");
        }
        PendingRequest::SessionNew { responder } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match session_id {
                Some(sid) => {
                    let _ = responder.send(Ok(sid));
                }
                None => {
                    let _ =
                        responder.send(Err("session/new response omitted sessionId".to_string()));
                }
            }
        }
        PendingRequest::SessionLoad {
            expected_session_id,
            responder,
        } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            // Some runtimes (kimi) omit sessionId from the session/load
            // response; fall back to what we sent.
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or(expected_session_id);
            let _ = responder.send(Ok(session_id));
        }
        PendingRequest::Prompt { session_id, run_id } => {
            let drained: Vec<(Option<String>, String, Value)> = {
                let mut s = shared.lock().unwrap();
                if let Some(slot) = s.sessions.get_mut(&session_id) {
                    let drained = slot.tool_accumulator.drain();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: session_id.clone(),
                    };
                    drained
                } else {
                    Vec::new()
                }
            };
            for (_id, name, input) in drained {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: session_id.clone(),
                    run_id,
                    item: AgentEventItem::ToolCall { name, input },
                });
            }
            let _ = event_tx.try_send(DriverEvent::Output {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                item: AgentEventItem::TurnEnd,
            });
            let _ = event_tx.try_send(DriverEvent::Completed {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            });
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: ProcessState::Active {
                    session_id: session_id.clone(),
                },
            });
        }
    }
}

fn handle_session_update(
    driver: &'static str,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    session_id_hint: Option<String>,
    items: Vec<AcpUpdateItem>,
) {
    for item in items {
        // Determine session id for this item. Normally comes from the
        // notification envelope; the SessionInit variant updates it.
        let mut sid_opt: Option<String> = None;
        {
            let s = shared.lock().unwrap();
            if let Some(ref hint) = session_id_hint {
                if s.sessions.contains_key(hint) {
                    sid_opt = Some(hint.clone());
                }
            }
            if sid_opt.is_none() && s.sessions.len() == 1 {
                sid_opt = s.sessions.keys().next().cloned();
            }
        }

        match item {
            AcpUpdateItem::SessionInit { session_id } => {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
            }
            AcpUpdateItem::Thinking { text } => {
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(driver, key, shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Thinking { text },
                    });
                }
            }
            AcpUpdateItem::Text { text } => {
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(driver, key, shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Text { text },
                    });
                }
            }
            AcpUpdateItem::ToolCall { id, name, input } => {
                if let Some(sid) = pick_session(driver, key, shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        let flushed = slot.tool_accumulator.drain();
                        let run_id = slot.run_id;
                        drop(s);
                        if let Some(run_id) = run_id {
                            for (_id, n, inp) in flushed {
                                let _ = event_tx.try_send(DriverEvent::Output {
                                    key: key.clone(),
                                    session_id: sid.clone(),
                                    run_id,
                                    item: AgentEventItem::ToolCall {
                                        name: n,
                                        input: inp,
                                    },
                                });
                            }
                        }
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            slot.tool_accumulator.record_call(id, name, input);
                        }
                    }
                }
            }
            AcpUpdateItem::ToolCallUpdate { id, input } => {
                if let Some(sid) = pick_session(driver, key, shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        slot.tool_accumulator.merge_update(id, input);
                    }
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                if let Some(sid) = pick_session(driver, key, shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::ToolResult { content },
                        });
                    }
                }
            }
            AcpUpdateItem::TurnEnd => {
                if let Some(sid) = pick_session(driver, key, shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::TurnEnd,
                        });
                    }
                }
            }
        }
    }
}

pub(super) fn pick_session(
    driver: &'static str,
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> Option<String> {
    let s = shared.lock().unwrap();
    if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            return Some(h.to_string());
        }
        // Hint present but not in sessions map — fall through to the
        // single-session heuristic, but LOUDLY. CLAUDE.md forbids silent
        // fallbacks; this makes the "stale hint" case visible so the real
        // cause (close raced with an update, parser returned a bogus sid)
        // gets diagnosed instead of masked.
        warn!(
            driver,
            agent = %key,
            hint = %h,
            session_count = s.sessions.len(),
            "pick_session hint missing from sessions — falling back to single-session heuristic"
        );
    }
    if s.sessions.len() == 1 {
        return s.sessions.keys().next().cloned();
    }
    if hint.is_none() && !s.sessions.is_empty() {
        warn!(
            driver,
            agent = %key,
            session_count = s.sessions.len(),
            "pick_session called with no hint and >1 live sessions — dropping update"
        );
    }
    None
}

pub(super) fn pick_session_and_run(
    driver: &'static str,
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> (Option<String>, Option<RunId>) {
    let s = shared.lock().unwrap();
    let sid = if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            Some(h.to_string())
        } else if s.sessions.len() == 1 {
            warn!(
                driver,
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "pick_session_and_run hint missing from sessions — falling back to single-session heuristic"
            );
            s.sessions.keys().next().cloned()
        } else {
            warn!(
                driver,
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "pick_session_and_run hint missing with ambiguous sessions — dropping update"
            );
            None
        }
    } else if s.sessions.len() == 1 {
        s.sessions.keys().next().cloned()
    } else {
        if !s.sessions.is_empty() {
            warn!(
                driver,
                agent = %key,
                session_count = s.sessions.len(),
                "pick_session_and_run called with no hint and >1 live sessions — dropping update"
            );
        }
        None
    };
    let run = sid
        .as_ref()
        .and_then(|id| s.sessions.get(id))
        .and_then(|slot| slot.run_id);
    (sid, run)
}
