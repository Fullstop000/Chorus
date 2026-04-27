//! Stateless Codex app-server JSON-RPC helpers.
//!
//! Pure parsing and encoding for the Codex app-server protocol. No process
//! lifecycle, no channels, no agent state — only data transformation.
//!
//! Wire format difference from ACP: the Codex app-server **omits** the
//! `"jsonrpc":"2.0"` field on the wire. All outgoing requests built here
//! do not include that field. The parser tolerates servers that do include
//! it (for defensive compatibility).
//!
//! Response routing is id-agnostic via [`parse_line_with_registry`]: it
//! classifies responses by the method the caller associated with the
//! request id rather than by id position. This supports multiplexing
//! multiple threads over one connection.

use serde_json::{json, Value};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed { message: String },
}

#[derive(Debug, Clone)]
pub enum ItemEvent {
    AgentMessage {
        id: String,
        text: String,
    },
    CommandExecution {
        id: String,
        command: String,
        cwd: Option<String>,
        exit_code: Option<i32>,
    },
    McpToolCall {
        id: String,
        server: String,
        tool: String,
        arguments: Value,
    },
    UserMessage {
        id: String,
    },
    Other {
        item_type: String,
        id: String,
    },
}

#[derive(Debug, Clone)]
pub enum AppServerEvent {
    // Responses (have an `id`)
    InitializeResponse,
    /// Fires when the server responds to `thread/start` (id 1).
    /// Contains `thread_id` which the caller **must cache** — it is required
    /// for all subsequent `turn/start` and `turn/interrupt` requests.
    ThreadResponse {
        thread_id: String,
    },
    /// Fires when the server responds to `turn/start` (id >= 2).
    /// Contains `turn_id` which the caller **must cache** — it is required
    /// to interrupt the turn via `turn/interrupt`.
    TurnResponse {
        turn_id: String,
    },
    /// Fires when the `turn/interrupt` response arrives (id >= 2, empty result).
    /// The turn is now cancelled; caller should stop forwarding deltas.
    TurnInterruptResponse,

    // Notifications (no `id`)
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted {
        turn_id: String,
    },
    TurnCompleted {
        turn_id: String,
        status: TurnStatus,
    },

    // Item lifecycle
    ItemStarted {
        item: ItemEvent,
    },
    ItemCompleted {
        item: ItemEvent,
    },

    // Deltas (streaming)
    AgentMessageDelta {
        item_id: String,
        text: String,
    },
    ReasoningSummaryDelta {
        item_id: String,
        text: String,
    },
    CommandOutputDelta {
        item_id: String,
        text: String,
    },

    // Approvals (server requests — have both `method` and `id`)
    /// Server requests approval before executing a shell command.
    /// Caller must respond with [`build_approval_response`] echoing `request_id`.
    /// Pass `"accept"` to allow or `"decline"` / `"cancel"` to deny.
    CommandApproval {
        request_id: Value,
        item_id: String,
        thread_id: String,
        turn_id: String,
    },
    /// Server requests approval before writing a file.
    /// Caller must respond with [`build_approval_response`] echoing `request_id`.
    /// Pass `"accept"` to allow or `"decline"` / `"cancel"` to deny.
    FileChangeApproval {
        request_id: Value,
        item_id: String,
        thread_id: String,
        turn_id: String,
    },

    // Error
    Error {
        id: Option<Value>,
        /// Method the caller registered for this request id, if any. Populated
        /// by consulting the registry on the error path too — required so the
        /// reader loop's closure always runs and removes the pending entry
        /// from its map (otherwise the caller's waker is never fired).
        /// `None` when the id was never registered or when the error has no id.
        method: Option<String>,
        /// JSON-RPC error code; `0` when absent.
        code: i64,
        message: String,
    },

    // Unknown / unrecognized
    Unknown,
}

// ---------------------------------------------------------------------------
// Private serialization helpers
// ---------------------------------------------------------------------------

/// Serialize a JSON-RPC request WITHOUT the `"jsonrpc"` field.
/// The Codex app-server wire format omits that header on the wire.
fn app_server_request(id: u64, method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("app_server_request serialization should not fail")
}

/// Serialize a JSON-RPC notification (no id) WITHOUT the `"jsonrpc"` field.
fn app_server_notification(method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "method": method,
        "params": params,
    }))
    .expect("app_server_notification serialization should not fail")
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Build the `initialize` request (id 0).
/// `clientInfo` identifies Chorus to the Codex compliance logs.
///
/// Schema match per `codex app-server generate-json-schema --experimental`
/// (verified codex-cli 0.125.0): `InitializeParams` accepts `clientInfo` +
/// `capabilities`. The earlier `clientCapabilities` key and `protocolVersion`
/// fields were not in the schema and were silently dropped.
pub fn build_initialize(id: u64) -> String {
    app_server_request(
        id,
        "initialize",
        json!({
            "clientInfo": {
                "name": "chorus",
                "title": "Chorus",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {},
        }),
    )
}

/// Build the `initialized` notification.
/// NOTE: This is a notification — no `id` field. Must be sent after the
/// `initialize` response is received to complete the handshake.
pub fn build_initialized() -> String {
    app_server_notification("initialized", json!({}))
}

/// Build a `thread/start` request.
/// Sets `approvalPolicy="never"`, `sandbox="danger-full-access"`, model, cwd.
///
/// `developer_instructions`, when present, rides as the `developerInstructions`
/// field. The earlier code put the system prompt in `personality`, but the
/// codex JSON schema defines `personality` as `enum: ["none", "friendly",
/// "pragmatic"]` (a tone selector), so a multi-thousand-character string was
/// rejected with `-32600 unknown variant`. `developerInstructions` is the
/// documented free-form slot.
pub fn build_thread_start(
    id: u64,
    model: &str,
    cwd: &str,
    developer_instructions: Option<&str>,
) -> String {
    let mut params = json!({
        "model": model,
        "cwd": cwd,
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
    });
    if let Some(prompt) = developer_instructions {
        params["developerInstructions"] = json!(prompt);
    }
    app_server_request(id, "thread/start", params)
}

/// Build a `thread/resume` request.
///
/// `developer_instructions` is forwarded so the standing prompt survives
/// resume (codex's `ThreadResumeParams` schema accepts the same field as
/// `ThreadStartParams`).
pub fn build_thread_resume(
    id: u64,
    thread_id: &str,
    developer_instructions: Option<&str>,
) -> String {
    let mut params = json!({ "threadId": thread_id });
    if let Some(prompt) = developer_instructions {
        params["developerInstructions"] = json!(prompt);
    }
    app_server_request(id, "thread/resume", params)
}

/// Build a `turn/start` request.
pub fn build_turn_start(id: u64, thread_id: &str, text: &str) -> String {
    app_server_request(
        id,
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": text }],
        }),
    )
}

/// Build a `turn/interrupt` request.
pub fn build_turn_interrupt(id: u64, thread_id: &str, turn_id: &str) -> String {
    app_server_request(
        id,
        "turn/interrupt",
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    )
}

/// Build an approval decision response. Echoes the server's `request_id`.
/// `decision` is a string like `"accept"`, `"decline"`, `"cancel"`.
/// NOTE: This is a JSON-RPC result response — no `method` field.
/// The bidirectional id spaces don't collide on the wire since this goes to
/// stdin while the reader task reads from stdout.
pub fn build_approval_response(request_id: &Value, decision: &str) -> String {
    serde_json::to_string(&json!({
        "id": request_id,
        "result": decision,
    }))
    .expect("build_approval_response serialization should not fail")
}

// ---------------------------------------------------------------------------
// Parse entry point
// ---------------------------------------------------------------------------

/// Parse one line of `codex app-server` stdout with id-agnostic response
/// routing. `method_for_id(id)` must return the method the caller
/// associated with a given numeric request id (typically via a
/// `HashMap<u64, _>`); notifications and server-initiated requests are
/// classified by method name directly.
///
/// Wire format: JSONL, no `"jsonrpc":"2.0"` header (omitted by app-server).
/// Parser tolerates messages that include `jsonrpc` for defensive compatibility.
///
/// Recognized methods for response classification:
///   * `"initialize"` → [`AppServerEvent::InitializeResponse`]
///   * `"thread/start"` or `"thread/resume"` → [`AppServerEvent::ThreadResponse`]
///   * `"turn/start"` → [`AppServerEvent::TurnResponse`]
///   * `"turn/interrupt"` → [`AppServerEvent::TurnInterruptResponse`]
///
/// Responses whose id is absent from the registry, or whose registered
/// method is unknown, return [`AppServerEvent::Unknown`]. Errors on a
/// registered id always produce [`AppServerEvent::Error`] regardless of
/// method.
pub fn parse_line_with_registry<F>(line: &str, method_for_id: F) -> AppServerEvent
where
    F: FnOnce(u64) -> Option<String>,
{
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return AppServerEvent::Unknown,
    };

    let has_id = msg.get("id").is_some();
    let has_result = msg.get("result").is_some();
    let has_error = msg.get("error").is_some();
    let has_method = msg.get("method").is_some();

    if has_id && (has_result || has_error) && !has_method {
        return parse_response_by_method(&msg, method_for_id);
    }

    if has_method && has_id {
        let method = msg["method"].as_str().unwrap_or("");
        return parse_server_request(method, &msg);
    }

    if has_method {
        let method = msg["method"].as_str().unwrap_or("");
        return parse_notification(method, &msg);
    }

    AppServerEvent::Unknown
}

fn parse_response_by_method<F>(msg: &Value, method_for_id: F) -> AppServerEvent
where
    F: FnOnce(u64) -> Option<String>,
{
    let id_val = msg.get("id");

    // Always consult `method_for_id` for responses carrying a numeric id —
    // including error responses. The reader loop's closure uses this call
    // to remove the pending request from its map (and capture the waker).
    // If we short-circuit here on the error path the pending entry leaks
    // and the caller's `rx.await` blocks forever.
    let id_u64 = id_val.and_then(|v| v.as_u64());
    let method = id_u64.and_then(method_for_id);

    if let Some(err) = msg.get("error") {
        let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        let message = err
            .get("data")
            .and_then(|d| d.get("message"))
            .and_then(|v| v.as_str())
            .or_else(|| err.get("message").and_then(|v| v.as_str()))
            .unwrap_or("unknown error")
            .to_string();
        return AppServerEvent::Error {
            id: id_val.cloned(),
            method,
            code,
            message,
        };
    }

    let Some(id_u64) = id_u64 else {
        return AppServerEvent::Unknown;
    };
    let result = msg.get("result");

    match method.as_deref() {
        Some("initialize") => AppServerEvent::InitializeResponse,
        Some("thread/start") | Some("thread/resume") => {
            let thread_id = result
                .and_then(|r| r.get("thread"))
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            match thread_id {
                Some(tid) => AppServerEvent::ThreadResponse { thread_id: tid },
                None => {
                    warn!(
                        method = method.as_deref(),
                        "codex app-server: thread response missing thread.id"
                    );
                    AppServerEvent::Unknown
                }
            }
        }
        Some("turn/start") => {
            let turn_id = result
                .and_then(|r| r.get("turn"))
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            match turn_id {
                Some(tid) => AppServerEvent::TurnResponse { turn_id: tid },
                None => {
                    warn!("codex app-server: turn/start response missing turn.id");
                    AppServerEvent::Unknown
                }
            }
        }
        Some("turn/interrupt") => AppServerEvent::TurnInterruptResponse,
        Some(other) => {
            warn!(
                method = other,
                "codex app-server: response for unregistered method"
            );
            AppServerEvent::Unknown
        }
        None => {
            warn!(
                id = id_u64,
                "codex app-server: response for unknown id — caller registry lookup failed"
            );
            AppServerEvent::Unknown
        }
    }
}

// ---------------------------------------------------------------------------
// Internal parse helpers
// ---------------------------------------------------------------------------

fn parse_notification(method: &str, msg: &Value) -> AppServerEvent {
    let params = msg.get("params").unwrap_or(&Value::Null);

    match method {
        "thread/started" => {
            let thread_id = params
                .get("thread")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppServerEvent::ThreadStarted { thread_id }
        }
        "turn/started" => {
            let turn_id = params
                .get("turn")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppServerEvent::TurnStarted { turn_id }
        }
        "turn/completed" => {
            let turn = params.get("turn").unwrap_or(&Value::Null);
            let turn_id = turn
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status_str = turn
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("completed");
            let status = match status_str {
                "completed" => TurnStatus::Completed,
                "interrupted" => TurnStatus::Interrupted,
                "failed" => {
                    let message = turn
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("turn failed")
                        .to_string();
                    TurnStatus::Failed { message }
                }
                other => {
                    warn!("codex app-server: unknown turn status: {other}");
                    TurnStatus::Failed {
                        message: format!("unknown status: {other}"),
                    }
                }
            };
            AppServerEvent::TurnCompleted { turn_id, status }
        }
        "item/started" => {
            let item = params.get("item").unwrap_or(&Value::Null);
            AppServerEvent::ItemStarted {
                item: parse_item(item),
            }
        }
        "item/completed" => {
            let item = params.get("item").unwrap_or(&Value::Null);
            AppServerEvent::ItemCompleted {
                item: parse_item(item),
            }
        }
        "item/agentMessage/delta" => {
            let item_id = params
                .get("itemId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // delta may be {"value": "text"} or a plain string
            let text = params
                .get("delta")
                .and_then(|d| {
                    if let Some(s) = d.as_str() {
                        Some(s.to_string())
                    } else {
                        d.get("value")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    }
                })
                .unwrap_or_default();
            AppServerEvent::AgentMessageDelta { item_id, text }
        }
        "item/reasoning/summaryTextDelta" => {
            let item_id = params
                .get("itemId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = params
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppServerEvent::ReasoningSummaryDelta { item_id, text }
        }
        "item/commandExecution/outputDelta" => {
            let item_id = params
                .get("itemId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // prefer "output" field; fall back to "delta"
            let text = params
                .get("output")
                .and_then(|v| v.as_str())
                .or_else(|| params.get("delta").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            AppServerEvent::CommandOutputDelta { item_id, text }
        }
        _ => {
            debug!(
                method = method,
                "codex app-server: unknown notification dropped"
            );
            AppServerEvent::Unknown
        }
    }
}

fn parse_server_request(method: &str, msg: &Value) -> AppServerEvent {
    // Invariant: parse_line_with_registry only calls this function when
    // has_id is true.
    let Some(request_id) = msg.get("id").cloned() else {
        return AppServerEvent::Unknown;
    };
    let params = msg.get("params").unwrap_or(&Value::Null);

    let item_id = params
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let thread_id = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let turn_id = params
        .get("turnId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match method {
        "item/commandExecution/requestApproval" => AppServerEvent::CommandApproval {
            request_id,
            item_id,
            thread_id,
            turn_id,
        },
        "item/fileChange/requestApproval" => AppServerEvent::FileChangeApproval {
            request_id,
            item_id,
            thread_id,
            turn_id,
        },
        _ => AppServerEvent::Unknown,
    }
}

fn parse_item(item: &Value) -> ItemEvent {
    let item_type = item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match item_type.as_str() {
        "agentMessage" => {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ItemEvent::AgentMessage { id, text }
        }
        "reasoning" | "fileChange" => ItemEvent::Other {
            item_type: item_type.clone(),
            id,
        },
        "commandExecution" => {
            let command = item
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cwd = item
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let exit_code = item
                .get("exitCode")
                .and_then(|v| v.as_i64())
                .map(|n| n as i32);
            ItemEvent::CommandExecution {
                id,
                command,
                cwd,
                exit_code,
            }
        }
        "mcpToolCall" => {
            let server = item
                .get("server")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool = item
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = item.get("arguments").cloned().unwrap_or_else(|| json!({}));
            ItemEvent::McpToolCall {
                id,
                server,
                tool,
                arguments,
            }
        }
        "userMessage" => ItemEvent::UserMessage { id },
        other => ItemEvent::Other {
            item_type: other.to_string(),
            id,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Builder tests
    // -----------------------------------------------------------------------

    #[test]
    fn build_initialize_has_no_jsonrpc_field() {
        let s = build_initialize(0);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("jsonrpc").is_none(), "jsonrpc field must be absent");
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["id"], 0);
        assert_eq!(v["params"]["clientInfo"]["name"], "chorus");
        // Schema match: codex InitializeParams uses `capabilities`, not
        // `clientCapabilities`. `protocolVersion` is not in the schema.
        assert!(v["params"].get("capabilities").is_some());
        assert!(v["params"].get("clientCapabilities").is_none());
        assert!(v["params"].get("protocolVersion").is_none());
    }

    #[test]
    fn build_initialized_is_notification() {
        let s = build_initialized();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("id").is_none(), "notifications must not have id");
        assert_eq!(v["method"], "initialized");
    }

    #[test]
    fn build_thread_start_shape() {
        let s = build_thread_start(1, "o4-mini", "/tmp", None);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("jsonrpc").is_none());
        assert_eq!(v["method"], "thread/start");
        assert_eq!(v["params"]["approvalPolicy"], "never");
        assert_eq!(v["params"]["sandbox"], "danger-full-access");
        assert_eq!(v["params"]["model"], "o4-mini");
        assert_eq!(v["params"]["cwd"], "/tmp");
    }

    #[test]
    fn build_thread_start_with_developer_instructions() {
        let s = build_thread_start(1, "o4-mini", "/tmp", Some("be helpful"));
        let v: Value = serde_json::from_str(&s).unwrap();
        // Schema match: free-form text rides as `developerInstructions`, NOT
        // `personality` (which is a 3-value enum: none|friendly|pragmatic).
        assert_eq!(v["params"]["developerInstructions"], "be helpful");
        assert!(v["params"].get("personality").is_none());
    }

    #[test]
    fn build_thread_start_without_developer_instructions() {
        let s = build_thread_start(1, "o4-mini", "/tmp", None);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v["params"].get("developerInstructions").is_none());
    }

    #[test]
    fn build_thread_resume_shape() {
        let s = build_thread_resume(1, "thr_123", None);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "thread/resume");
        assert_eq!(v["params"]["threadId"], "thr_123");
        assert!(v.get("jsonrpc").is_none());
        assert!(v["params"].get("developerInstructions").is_none());
    }

    #[test]
    fn build_thread_resume_carries_developer_instructions() {
        let s = build_thread_resume(2, "thr_abc", Some("standing prompt"));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["params"]["threadId"], "thr_abc");
        assert_eq!(v["params"]["developerInstructions"], "standing prompt");
    }

    #[test]
    fn build_turn_start_shape() {
        let s = build_turn_start(2, "thr_123", "hello");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "turn/start");
        assert_eq!(v["params"]["threadId"], "thr_123");
        assert_eq!(v["params"]["input"][0]["type"], "text");
        assert_eq!(v["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn build_turn_interrupt_shape() {
        let s = build_turn_interrupt(3, "thr_123", "turn_456");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "turn/interrupt");
        assert_eq!(v["params"]["threadId"], "thr_123");
        assert_eq!(v["params"]["turnId"], "turn_456");
    }

    #[test]
    fn build_approval_response_accept() {
        let request_id = json!(42u64);
        let s = build_approval_response(&request_id, "accept");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("method").is_none(), "must not have method field");
        assert_eq!(v["result"], "accept");
        assert_eq!(v["id"], 42u64);
    }

    #[test]
    fn build_approval_response_tolerates_json_value_id() {
        // id can be a string or number
        let request_id_str = json!("req-abc");
        let s = build_approval_response(&request_id_str, "decline");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], "req-abc");
        assert_eq!(v["result"], "decline");

        let request_id_num = json!(99u64);
        let s2 = build_approval_response(&request_id_num, "cancel");
        let v2: Value = serde_json::from_str(&s2).unwrap();
        assert_eq!(v2["id"], 99u64);
        assert_eq!(v2["result"], "cancel");
    }

    // -----------------------------------------------------------------------
    // parse_line_with_registry — id-agnostic response classification
    // -----------------------------------------------------------------------

    #[test]
    fn registry_thread_start_response_uses_method() {
        // id 42 is outside the legacy-id range (0/1/2+) but resolves via registry.
        let line = r#"{"id":42,"result":{"thread":{"id":"thr_abc"}}}"#;
        let ev = parse_line_with_registry(line, |id| {
            assert_eq!(id, 42);
            Some("thread/start".into())
        });
        match ev {
            AppServerEvent::ThreadResponse { thread_id } => {
                assert_eq!(thread_id, "thr_abc");
            }
            other => panic!("expected ThreadResponse, got {other:?}"),
        }
    }

    #[test]
    fn registry_thread_resume_is_same_variant() {
        let line = r#"{"id":100,"result":{"thread":{"id":"thr_xyz"}}}"#;
        let ev = parse_line_with_registry(line, |_| Some("thread/resume".into()));
        match ev {
            AppServerEvent::ThreadResponse { thread_id } => {
                assert_eq!(thread_id, "thr_xyz");
            }
            other => panic!("expected ThreadResponse, got {other:?}"),
        }
    }

    #[test]
    fn registry_turn_start_response() {
        let line =
            r#"{"id":7,"result":{"turn":{"id":"turn_99","status":"inProgress","items":[]}}}"#;
        let ev = parse_line_with_registry(line, |_| Some("turn/start".into()));
        match ev {
            AppServerEvent::TurnResponse { turn_id } => assert_eq!(turn_id, "turn_99"),
            other => panic!("expected TurnResponse, got {other:?}"),
        }
    }

    #[test]
    fn registry_turn_interrupt_response() {
        let line = r#"{"id":8,"result":{}}"#;
        let ev = parse_line_with_registry(line, |_| Some("turn/interrupt".into()));
        assert!(matches!(ev, AppServerEvent::TurnInterruptResponse));
    }

    #[test]
    fn registry_initialize_response() {
        let line = r#"{"id":5,"result":{"protocolVersion":1}}"#;
        let ev = parse_line_with_registry(line, |_| Some("initialize".into()));
        assert!(matches!(ev, AppServerEvent::InitializeResponse));
    }

    #[test]
    fn registry_error_path_independent_of_method() {
        let line = r#"{"id":9,"error":{"code":-32000,"message":"nope"}}"#;
        let ev = parse_line_with_registry(line, |_| Some("thread/start".into()));
        match ev {
            AppServerEvent::Error { message, .. } => assert_eq!(message, "nope"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn registry_error_path_consults_method_for_id() {
        // Regression: the error path MUST call `method_for_id(id)` so the
        // reader-loop closure removes the pending entry and captures the
        // waker. Prior to the fix this branch short-circuited before the
        // closure ran, leaking the entry and leaving callers stuck on
        // `rx.await` forever. Verify the closure fires and the method is
        // populated on the resulting Error event.
        use std::cell::Cell;
        let called = Cell::new(false);
        let line =
            r#"{"id":77,"error":{"code":-32600,"message":"no rollout found for thread id"}}"#;
        let ev = parse_line_with_registry(line, |id| {
            assert_eq!(id, 77);
            called.set(true);
            Some("thread/resume".into())
        });
        assert!(called.get(), "method_for_id must be called on error path");
        match ev {
            AppServerEvent::Error {
                method,
                code,
                message,
                ..
            } => {
                assert_eq!(method.as_deref(), Some("thread/resume"));
                assert_eq!(code, -32600);
                assert!(message.contains("no rollout"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn registry_unknown_id_returns_unknown() {
        // Registry lookup failure (unseen id) must produce Unknown rather
        // than misclassifying by id heuristic.
        let line = r#"{"id":999,"result":{"thread":{"id":"thr_abc"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        assert!(
            matches!(ev, AppServerEvent::Unknown),
            "unknown id should yield Unknown, got {ev:?}"
        );
    }

    #[test]
    fn registry_forwards_notifications_unchanged() {
        // Notifications have no id and should follow the notification path
        // regardless of the registry closure (which should never be called).
        let line =
            r#"{"method":"turn/completed","params":{"turn":{"id":"turn_x","status":"completed"}}}"#;
        let ev = parse_line_with_registry(line, |_| panic!("registry must not be called"));
        assert!(matches!(ev, AppServerEvent::TurnCompleted { .. }));
    }

    #[test]
    fn registry_forwards_server_requests_unchanged() {
        let line = r#"{"method":"item/commandExecution/requestApproval","id":42,"params":{"itemId":"item_1","threadId":"thr_x","turnId":"turn_y"}}"#;
        let ev = parse_line_with_registry(line, |_| {
            // server-initiated requests are classified by method name, not id
            panic!("registry must not be called for server requests")
        });
        assert!(matches!(ev, AppServerEvent::CommandApproval { .. }));
    }

    #[test]
    fn parse_thread_started_notification() {
        let line = r#"{"method":"thread/started","params":{"thread":{"id":"thr_123"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ThreadStarted { thread_id } => {
                assert_eq!(thread_id, "thr_123");
            }
            other => panic!("expected ThreadStarted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_started_notification() {
        let line = r#"{"method":"turn/started","params":{"turn":{"id":"turn_456"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::TurnStarted { turn_id } => {
                assert_eq!(turn_id, "turn_456");
            }
            other => panic!("expected TurnStarted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_natural() {
        let line = r#"{"method":"turn/completed","params":{"turn":{"id":"turn_456","status":"completed"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::TurnCompleted { turn_id, status } => {
                assert_eq!(turn_id, "turn_456");
                assert_eq!(status, TurnStatus::Completed);
            }
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_interrupted() {
        let line = r#"{"method":"turn/completed","params":{"turn":{"id":"turn_456","status":"interrupted"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::TurnCompleted { status, .. } => {
                assert_eq!(status, TurnStatus::Interrupted);
            }
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_failed() {
        let line = r#"{"method":"turn/completed","params":{"turn":{"id":"turn_456","status":"failed","error":{"message":"out of context"}}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::TurnCompleted { status, .. } => {
                assert!(
                    matches!(status, TurnStatus::Failed { message } if message == "out of context")
                );
            }
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_item_started_agent_message() {
        let line = r#"{"method":"item/started","params":{"item":{"type":"agentMessage","id":"item_1","text":""}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ItemStarted {
                item: ItemEvent::AgentMessage { id, .. },
            } => {
                assert_eq!(id, "item_1");
            }
            other => panic!("expected ItemStarted(AgentMessage), got {other:?}"),
        }
    }

    #[test]
    fn parse_item_completed_command_execution() {
        let line = r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"item_2","command":"ls","exitCode":0}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ItemCompleted {
                item: ItemEvent::CommandExecution { exit_code, .. },
            } => {
                assert_eq!(exit_code, Some(0));
            }
            other => panic!("expected ItemCompleted(CommandExecution), got {other:?}"),
        }
    }

    #[test]
    fn parse_item_completed_file_change() {
        let line = r#"{"method":"item/completed","params":{"item":{"type":"fileChange","id":"item_3","changes":[{"path":"foo.rs","kind":"modify","diff":"..."}]}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ItemCompleted {
                item: ItemEvent::Other { item_type, id },
            } => {
                assert_eq!(item_type, "fileChange");
                assert_eq!(id, "item_3");
            }
            other => panic!("expected ItemCompleted(Other), got {other:?}"),
        }
    }

    #[test]
    fn parse_item_completed_mcp_tool_call() {
        let line = r#"{"method":"item/completed","params":{"item":{"type":"mcpToolCall","id":"item_4","server":"my-server","tool":"do_thing","arguments":{"k":"v"}}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ItemCompleted {
                item: ItemEvent::McpToolCall { server, tool, .. },
            } => {
                assert_eq!(server, "my-server");
                assert_eq!(tool, "do_thing");
            }
            other => panic!("expected ItemCompleted(McpToolCall), got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_message_delta() {
        let line = r#"{"method":"item/agentMessage/delta","params":{"itemId":"item_1","delta":{"value":"hello"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::AgentMessageDelta { item_id, text } => {
                assert_eq!(item_id, "item_1");
                assert_eq!(text, "hello");
            }
            other => panic!("expected AgentMessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_reasoning_summary_delta() {
        let line = r#"{"method":"item/reasoning/summaryTextDelta","params":{"itemId":"item_1","delta":"think"}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ReasoningSummaryDelta { item_id, text } => {
                assert_eq!(item_id, "item_1");
                assert_eq!(text, "think");
            }
            other => panic!("expected ReasoningSummaryDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_output_delta() {
        let line = r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"item_1","output":"line\n"}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::CommandOutputDelta { item_id, text } => {
                assert_eq!(item_id, "item_1");
                assert_eq!(text, "line\n");
            }
            other => panic!("expected CommandOutputDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_approval_server_request() {
        let line = r#"{"method":"item/commandExecution/requestApproval","id":42,"params":{"itemId":"item_1","threadId":"thr_123","turnId":"turn_456"}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::CommandApproval {
                request_id,
                item_id,
                thread_id,
                turn_id,
            } => {
                assert_eq!(request_id, json!(42u64));
                assert_eq!(item_id, "item_1");
                assert_eq!(thread_id, "thr_123");
                assert_eq!(turn_id, "turn_456");
            }
            other => panic!("expected CommandApproval, got {other:?}"),
        }
    }

    #[test]
    fn parse_file_change_approval_server_request() {
        let line = r#"{"method":"item/fileChange/requestApproval","id":43,"params":{"itemId":"item_5","threadId":"thr_abc","turnId":"turn_xyz"}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::FileChangeApproval {
                item_id,
                thread_id,
                turn_id,
                ..
            } => {
                assert_eq!(item_id, "item_5");
                assert_eq!(thread_id, "thr_abc");
                assert_eq!(turn_id, "turn_xyz");
            }
            other => panic!("expected FileChangeApproval, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_line() {
        let ev = parse_line_with_registry("", |_| None);
        assert!(matches!(ev, AppServerEvent::Unknown));
    }

    #[test]
    fn parse_invalid_json() {
        let ev = parse_line_with_registry("not json {{{", |_| None);
        assert!(matches!(ev, AppServerEvent::Unknown));
    }

    #[test]
    fn parse_unknown_notification() {
        let line = r#"{"method":"some/unknown","params":{}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        assert!(matches!(ev, AppServerEvent::Unknown));
    }

    #[test]
    fn parse_item_other_type() {
        let line = r#"{"method":"item/completed","params":{"item":{"type":"collabToolCall","id":"item_9"}}}"#;
        let ev = parse_line_with_registry(line, |_| None);
        match ev {
            AppServerEvent::ItemCompleted {
                item: ItemEvent::Other { item_type, .. },
            } => {
                assert_eq!(item_type, "collabToolCall");
            }
            other => panic!("expected ItemCompleted(Other), got {other:?}"),
        }
    }

    #[test]
    fn test_parse_agent_message_delta_string_form() {
        // "delta" can be a plain string instead of {"value": ...}
        let line =
            r#"{"method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"hello text"}}"#;
        match parse_line_with_registry(line, |_| None) {
            AppServerEvent::AgentMessageDelta { item_id, text } => {
                assert_eq!(item_id, "m1");
                assert_eq!(text, "hello text");
            }
            other => panic!("expected AgentMessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_command_output_delta_fallback() {
        // falls back to "delta" field when "output" is absent
        let line = r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"cmd1","delta":"output text"}}"#;
        match parse_line_with_registry(line, |_| None) {
            AppServerEvent::CommandOutputDelta { item_id, text } => {
                assert_eq!(item_id, "cmd1");
                assert_eq!(text, "output text");
            }
            other => panic!("expected CommandOutputDelta, got {:?}", other),
        }
    }
}
