//! Stateless ACP (Agent Client Protocol) JSON-RPC helpers.
//!
//! Pure parsing and encoding for the Agent Client Protocol. Per-runtime
//! transports (KimiAcpNativeTransport, ClaudeAcpAdapterTransport, ...) own
//! their own phase-tracking state and tool-call accumulator; this module
//! emits only what the raw wire frames say.
//!
//! Source of truth: the v1 handler at `src/agent/drivers/acp.rs`. Every
//! frame shape this parser recognizes must match a shape v1 recognizes —
//! divergence means a later transport will hit a parse failure.
//!
//! Request-id convention (inherited from v1):
//!   id 1  = initialize
//!   id 2  = session/new or session/load
//!   id 3  = initial session/prompt
//!   id >= 4 = follow-up session/prompts
//!
//! The caller tracks its own `AcpPhase` and owns the next-id allocator.

use serde_json::{json, Value};
use tracing::{debug, trace, warn};

// ---------------------------------------------------------------------------
// MCP prefix stripping (moved from deleted v1 acp.rs)
// ---------------------------------------------------------------------------

/// Strips known MCP/chat prefixes from tool names so the activity log shows
/// short human-readable names like `send_message` instead of
/// `mcp__chat__send_message`.
pub(crate) fn strip_mcp_prefix(name: &str) -> &str {
    // Standard MCP prefix forms: mcp__chat__, mcp_chat_, chat_
    if let Some(s) = name
        .strip_prefix("mcp__chat__")
        .or_else(|| name.strip_prefix("mcp_chat_"))
        .or_else(|| name.strip_prefix("chat_"))
    {
        return s;
    }
    // Claude ACP formats tool titles as "Tool: <server>/<tool_name>"
    // e.g. "Tool: chat/send_message" → "send_message"
    if name.starts_with("Tool: ") {
        if let Some(slash) = name.find('/') {
            return &name[slash + 1..];
        }
    }
    name
}

// ---------------------------------------------------------------------------
// Protocol phase state machine
// ---------------------------------------------------------------------------

/// ACP handshake phase. The caller (a concrete transport) mutates this in
/// response to `AcpParsed::InitializeResponse` and `AcpParsed::SessionResponse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPhase {
    /// Waiting for the `initialize` response (id 1).
    AwaitingInitResponse,
    /// Waiting for the `session/new` or `session/load` response (id 2).
    AwaitingSessionResponse,
    /// Handshake complete; parsing `session/update` notifications and
    /// `session/prompt` responses.
    Active,
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Build the `initialize` request. Caller provides the JSON-RPC id (should be 1).
/// Includes the same clientInfo + empty capabilities payload v1 sends.
pub fn build_initialize_request(id: u64) -> String {
    json_rpc_request(
        id,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientInfo": {
                "name": "chorus",
                "title": "Chorus",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "clientCapabilities": {},
        }),
    )
}

/// Build the `session/new` request. `params` is runtime-specific (typically
/// `{ "workspaceDir": "/path" }` but kimi and others may extend it).
pub fn build_session_new_request(id: u64, params: Value) -> String {
    json_rpc_request(id, "session/new", params)
}

/// Build the `session/load` request for resuming a session. `sessionId` is
/// merged into `params` so callers don't have to remember to splice it in.
/// Callers should remember the id they sent — some runtimes (kimi) omit it
/// from the response and the transport must fall back to the requested value.
pub fn build_session_load_request(id: u64, session_id: &str, mut params: Value) -> String {
    if let Some(obj) = params.as_object_mut() {
        obj.insert(
            "sessionId".to_string(),
            Value::String(session_id.to_string()),
        );
    }
    json_rpc_request(id, "session/load", params)
}

/// Build a `session/prompt` request. `session_id` is embedded when non-empty;
/// runtimes that don't require it (e.g., claude ACP adapter) can pass `""`.
pub fn build_session_prompt_request(id: u64, session_id: &str, prompt_text: &str) -> String {
    let mut params = json!({
        "prompt": [{ "type": "text", "text": prompt_text }],
    });
    if !session_id.is_empty() {
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "sessionId".to_string(),
                Value::String(session_id.to_string()),
            );
        }
    }
    json_rpc_request(id, "session/prompt", params)
}

/// Build a response to `session/request_permission`. When `approved` is true
/// we select `approve` as the option id (matches v1's unknown-option fallback);
/// when false we emit a `cancelled` outcome so the runtime surfaces the denial.
///
/// Note: v1's `handle_rpc_request` inspects the request's `options[]` and
/// prefers `allow_always` then `allow_once`. That policy belongs in the
/// transport (which has the request JSON) — this builder just encodes the
/// caller's chosen option id. Callers that approve unconditionally pass
/// `true` and get the generic `"approve"` option id; callers that parsed the
/// request should use `build_permission_response_raw` with the real option.
pub fn build_permission_approval_response(request_id: u64, approved: bool) -> String {
    let body = if approved {
        json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "outcome": {
                    "outcome": "selected",
                    "optionId": "approve",
                },
            },
        })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "outcome": {
                    "outcome": "cancelled",
                },
            },
        })
    };
    serde_json::to_string(&body).expect("permission response serialization should not fail")
}

/// Build a `session/request_permission` response with the transport's chosen
/// option id. Lets transports mirror v1's "prefer allow_always then allow_once"
/// policy by forwarding the option id they extracted from the request.
pub fn build_permission_response_raw(request_id: u64, option_id: &str) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "outcome": {
                "outcome": "selected",
                "optionId": option_id,
            },
        },
    });
    serde_json::to_string(&body).expect("permission response serialization should not fail")
}

// ---------------------------------------------------------------------------
// Parser output
// ---------------------------------------------------------------------------

/// Parsed view of a single ACP line.
#[derive(Debug, Clone)]
pub enum AcpParsed {
    /// `initialize` response (id 1). Caller advances phase to
    /// `AwaitingSessionResponse`.
    InitializeResponse,
    /// `session/new` or `session/load` response (id 2). `session_id` is
    /// `None` when the response omits it (kimi's session/load does this);
    /// callers should fall back to whatever they sent in the request.
    SessionResponse { session_id: Option<String> },
    /// `session/prompt` response (id >= 3). Caller treats this as turn end.
    PromptResponse { session_id: Option<String> },
    /// `session/update` notification. One wire frame can yield multiple items
    /// (e.g., a `tool_call_update` carries both a rawInput update and a
    /// `content` tool result).
    SessionUpdate { items: Vec<AcpUpdateItem> },
    /// `session/request_permission` incoming request. `request_id` is the
    /// JSON-RPC id the transport must echo in its response.
    ///
    /// `options` carries the approval choices offered by the runtime (e.g.
    /// `allow_always`, `allow_once`). Transports should call
    /// [`pick_best_option_id`] to select the most permissive option and
    /// respond with [`build_permission_response_raw`] using that id.
    PermissionRequested {
        request_id: u64,
        tool_name: Option<String>,
        /// The approval options the runtime is willing to accept. Empty when
        /// the runtime omits the field (some kimi versions do this).
        options: Vec<PermissionOption>,
    },
    /// JSON-RPC error response (any id). Surfaces to the transport; v1
    /// policy is to emit it and let the caller decide whether to tear down.
    Error { message: String },
    /// The line parsed as JSON but was not a recognizable ACP frame — skip it.
    Unknown,
}

/// A single approval option from a `session/request_permission` request.
///
/// Each option has a `kind` (e.g. `"allow_always"`, `"allow_once"`) and an
/// `optionId` that must be echoed back verbatim in the response. Runtimes
/// reject responses whose `optionId` doesn't match any offered option — this
/// was the root cause of claude-agent-acp's "User refused permission" bug.
#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub kind: String,
    pub option_id: String,
}

/// Pick the best option id from a list of permission options.
///
/// Mirrors v1's policy: prefer `allow_always` (session-wide approval) →
/// `allow_once` → first available option → fallback `"approve"`. The last
/// fallback covers runtimes that omit `options[]` entirely (some kimi
/// versions), where the hardcoded `"approve"` happens to work.
pub fn pick_best_option_id(options: &[PermissionOption]) -> &str {
    options
        .iter()
        .find(|o| o.kind == "allow_always")
        .or_else(|| options.iter().find(|o| o.kind == "allow_once"))
        .map(|o| o.option_id.as_str())
        .or_else(|| options.first().map(|o| o.option_id.as_str()))
        .unwrap_or("approve")
}

/// An item extracted from a `session/update` notification.
///
/// The `ToolCall` variant carries the tool id (when present) so transports
/// can merge deferred `tool_call_update` frames via [`ToolCallAccumulator`].
/// `ToolResult` and `TurnEnd` consume no id.
#[derive(Debug, Clone)]
pub enum AcpUpdateItem {
    /// Session id was attached by a `session_init`-style update. Most runtimes
    /// deliver the session id via the `session/new` response; this variant
    /// covers the few that emit it inside an update instead.
    SessionInit { session_id: String },
    /// Agent reasoning chunk (`agent_thought_chunk` / `agentThoughtChunk`).
    Thinking { text: String },
    /// Agent-facing text chunk (`agent_message_chunk` / `agentMessageChunk`).
    Text { text: String },
    /// Initial tool-call announcement. `id` is the runtime's `toolCallId`
    /// when present; `input` is whichever of `args`, `rawInput`, or `input`
    /// was populated (may be `Value::Null`).
    ToolCall {
        id: Option<String>,
        name: String,
        input: Value,
    },
    /// Deferred-args update for a previously-announced tool call. Transports
    /// must merge this into the matching `ToolCall` (match by `id`) before
    /// surfacing to consumers.
    ToolCallUpdate { id: Option<String>, input: Value },
    /// Tool-call completion content (either a bare `content` string or a
    /// structured content array, both of which v1 normalizes to a joined
    /// text blob).
    ToolResult { content: String },
    /// End-of-turn marker. Not currently emitted from `session/update`
    /// (turn end comes from the `session/prompt` response) but kept in the
    /// enum so a future runtime that inlines it doesn't require a parser
    /// change.
    TurnEnd,
}

// ---------------------------------------------------------------------------
// Parse entry point
// ---------------------------------------------------------------------------

/// Parse one line of runtime stdout as an ACP JSON-RPC message.
///
/// Returns `AcpParsed::Unknown` for:
///   - empty lines
///   - lines that don't parse as JSON
///   - JSON that isn't a recognized ACP frame
///
/// This function is stateless; the caller tracks `AcpPhase` separately.
pub fn parse_line(line: &str) -> AcpParsed {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return AcpParsed::Unknown,
    };

    // JSON-RPC response: has "id" AND ("result" OR "error").
    if msg.get("id").is_some() && (msg.get("result").is_some() || msg.get("error").is_some()) {
        return parse_response(&msg);
    }

    // JSON-RPC request or notification: has "method".
    if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
        if msg.get("id").is_some() {
            // Server-initiated request; currently only permission asks.
            return parse_server_request(method, &msg);
        }
        // Notification.
        return parse_notification(method, &msg);
    }

    AcpParsed::Unknown
}

fn parse_response(msg: &Value) -> AcpParsed {
    if let Some(err) = msg.get("error") {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown ACP error")
            .to_string();
        return AcpParsed::Error { message };
    }

    let id = msg.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let result = msg.get("result");

    // Dispatch by id. v1 uses the same ids: 1 = initialize, 2 = session/new|load,
    // >= 3 = session/prompt. Ids that don't match these buckets fall through to
    // Unknown so the caller can decide whether to log.
    match id {
        1 => AcpParsed::InitializeResponse,
        2 => {
            let session_id = result
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            AcpParsed::SessionResponse { session_id }
        }
        n if n >= 3 => {
            let session_id = result
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            AcpParsed::PromptResponse { session_id }
        }
        _ => AcpParsed::Unknown,
    }
}

fn parse_server_request(method: &str, msg: &Value) -> AcpParsed {
    if method != "session/request_permission" {
        return AcpParsed::Unknown;
    }

    let Some(request_id) = msg.get("id").and_then(|v| v.as_u64()) else {
        // Spec requires an id here; without it we cannot respond. Treat as
        // unknown rather than synthesizing a default — silent fallbacks are
        // forbidden (CLAUDE.md: "fix root causes, not symptoms").
        return AcpParsed::Unknown;
    };

    // v1 extracts the bare tool name from `toolCall.title`, splitting on the
    // first ':' (titles look like `"send_message: {...}"`).
    let tool_name = msg
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("title"))
        .and_then(|t| t.as_str())
        .map(|t| t.split(':').next().unwrap_or(t).trim().to_string());

    // Extract approval options so transports can echo the correct optionId.
    // Without this, hardcoding "approve" causes claude-agent-acp to reject
    // the response as unrecognized ("User refused permission to run tool").
    let options = msg
        .get("params")
        .and_then(|p| p.get("options"))
        .and_then(|o| o.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|opt| {
                    let kind = opt.get("kind")?.as_str()?.to_string();
                    let option_id = opt.get("optionId")?.as_str()?.to_string();
                    Some(PermissionOption { kind, option_id })
                })
                .collect()
        })
        .unwrap_or_default();

    AcpParsed::PermissionRequested {
        request_id,
        tool_name,
        options,
    }
}

fn parse_notification(method: &str, msg: &Value) -> AcpParsed {
    if method != "session/update" {
        return AcpParsed::Unknown;
    }

    let Some(params) = msg.get("params") else {
        // No params at all — fully malformed frame. Return an empty items
        // vec (no SessionInit) so drivers see a no-op rather than routing
        // confusion. Emitting an empty-string SessionInit here would route
        // the update to a nonexistent session (Kimi would insert "" as a
        // session_id key; OpenCode would drop the update). The `warn!`
        // surfaces the spec violation for triage.
        warn!("acp session/update missing params entirely — emitting empty items vec");
        return AcpParsed::SessionUpdate { items: vec![] };
    };

    // Per ACP spec (https://agentclientprotocol.com/protocol/session-setup),
    // every `session/update` notification carries `params.sessionId`. When
    // present, we prepend an `AcpUpdateItem::SessionInit { session_id }` at
    // position 0 so multi-session drivers can route deterministically by
    // the first item rather than falling back to HashMap iteration order
    // of in-flight sessions.
    //
    // Missing/non-string sessionId is a malformed frame per spec. Rather
    // than emitting a `SessionInit { session_id: "" }` (which would route
    // the update to a nonexistent session), we omit the `SessionInit`
    // entirely and return the body items alone. Drivers without a
    // `SessionInit` at `items[0]` already fall back to their `pick_session`
    // heuristics (with warn-on-ambiguity logging). The `warn!` here
    // surfaces the spec violation for triage.
    let session_id = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if session_id.is_none() {
        warn!(
            "acp session/update missing params.sessionId — spec violation, emitting body items without SessionInit"
        );
    }

    // Some runtimes wrap updates in `params.update`; others put fields at the
    // top level of `params`. v1 handles both with a fallback.
    let update = params.get("update").unwrap_or(params);

    // `kind` can live in three places:
    //   - kimi / opencode:  update.sessionUpdate ("tool_call", "agent_message_chunk", ...)
    //   - claude ACP:       update.kind  or  update.type
    // v1 note: opencode ALSO emits `update.kind` for tool category ("read",
    // "write", "other") which is NOT an event type — so `sessionUpdate` wins.
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("kind"))
        .or_else(|| update.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let body_items: Vec<AcpUpdateItem> = match kind {
        "agentMessageChunk" | "agent_message_chunk" => {
            let text = extract_text(update);
            if text.is_empty() {
                vec![]
            } else {
                vec![AcpUpdateItem::Text { text }]
            }
        }
        "agentThoughtChunk" | "agent_thought_chunk" => {
            let text = extract_text(update);
            if text.is_empty() {
                vec![]
            } else {
                vec![AcpUpdateItem::Thinking { text }]
            }
        }
        "toolCall" | "tool_call" => {
            // kimi uses `title`; others use `toolName`.
            let raw_name = update
                .get("toolName")
                .or_else(|| update.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_tool");
            let name = strip_mcp_prefix(raw_name).to_string();

            let input = update
                .get("args")
                .or_else(|| update.get("rawInput"))
                .or_else(|| update.get("input"))
                .cloned()
                .unwrap_or(Value::Null);

            let id = update
                .get("toolCallId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            trace!(tool = %name, "acp tool call");
            vec![AcpUpdateItem::ToolCall { id, name, input }]
        }
        "toolCallUpdate" | "tool_call_update" => parse_tool_call_update(update),
        // Informational kinds v1 intentionally ignores. Listed so the catch-all
        // `_` arm (which logs a debug) doesn't fire on routine traffic.
        "userMessageChunk"
        | "user_message_chunk"
        | "plan"
        | "availableCommandsUpdate"
        | "available_commands_update"
        | "currentModeUpdate"
        | "current_mode_update"
        | "configOptionUpdate"
        | "config_option_update"
        | "sessionInfoUpdate"
        | "session_info_update"
        | "" => vec![],
        _ => {
            debug!(
                kind,
                "acp session/update: unrecognized kind — update dropped"
            );
            vec![]
        }
    };

    // When sessionId is present, prepend SessionInit at position 0. Drivers
    // iterate and stop at the first SessionInit for routing — position is
    // load-bearing. When sessionId is absent (malformed frame), we emit the
    // body items only; drivers fall back to their `pick_session` heuristics.
    let items = match session_id {
        Some(session_id) => {
            let mut items = Vec::with_capacity(body_items.len() + 1);
            items.push(AcpUpdateItem::SessionInit { session_id });
            items.extend(body_items);
            items
        }
        None => body_items,
    };

    AcpParsed::SessionUpdate { items }
}

fn parse_tool_call_update(update: &Value) -> Vec<AcpUpdateItem> {
    let mut out = Vec::new();

    let id = update
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Deferred-args update: ACP runtimes often send `tool_call` with empty
    // args, then deliver the real input via `rawInput` in a later update.
    if let Some(raw_input) = update.get("rawInput").or_else(|| update.get("args")) {
        if !raw_input.is_null() && *raw_input != Value::Object(Default::default()) {
            trace!("acp tool call input update");
            out.push(AcpUpdateItem::ToolCallUpdate {
                id: id.clone(),
                input: raw_input.clone(),
            });
        }
    }

    // Tool result: either a plain string in `content`, or a structured array.
    let content_str = update.get("content").and_then(|v| v.as_str());
    if let Some(text) = content_str {
        if !text.is_empty() {
            trace!("acp tool result");
            out.push(AcpUpdateItem::ToolResult {
                content: text.to_string(),
            });
        }
    } else if let Some(arr) = update.get("content").and_then(|v| v.as_array()) {
        // Structured content. Two observed shapes:
        //   - kimi: [{"content": {"text": "...", "type": "text"}, "type": "content"}]
        //   - flat: [{"type": "text", "text": "..."}]
        let text: String = arr
            .iter()
            .filter_map(|b| {
                b.get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                            b.get("text").and_then(|v| v.as_str()).map(str::to_string)
                        } else {
                            None
                        }
                    })
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            trace!("acp tool result (structured)");
            out.push(AcpUpdateItem::ToolResult { content: text });
        }
    }

    out
}

/// Extract text from an update node that may expose it as a plain string
/// field (`chunk`, `text`) or a nested object (`content.text`).
fn extract_text(update: &Value) -> String {
    update
        .get("chunk")
        .or_else(|| update.get("text"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            update
                .get("content")
                .and_then(|c| c.get("text").and_then(|v| v.as_str()).map(str::to_string))
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tool-call merge accumulator
// ---------------------------------------------------------------------------

/// Buffers `ToolCall`s and merges any subsequent `ToolCallUpdate` frames that
/// share the same tool id into the pending call's input.
///
/// Policy: ALWAYS defer. Every `record_call` goes into the pending list and
/// is flushed by `drain`. This mirrors v1's implicit buffering via
/// `last_tool_raw_name` — callers see a single fully-inputted tool call even
/// when the runtime streamed args in a second update.
///
/// `drain` returns the pending calls in insertion order and clears the state.
/// Transports typically drain on `TurnEnd` (v1's `session/prompt` response)
/// or immediately before emitting a new call if they want eager flushes.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    pending: Vec<(Option<String>, String, Value)>,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new tool call. Always deferred.
    pub fn record_call(&mut self, id: Option<String>, name: String, input: Value) {
        self.pending.push((id, name, input));
    }

    /// Merge an update. Matches the most recent pending call with the same
    /// non-null id. Orphan updates (no id match) are logged and dropped.
    ///
    /// We walk in reverse so back-to-back `ToolCall`+`ToolCallUpdate` from
    /// the same call collapse correctly even when multiple calls are pending.
    pub fn merge_update(&mut self, id: Option<String>, input: Value) {
        if let Some(ref target_id) = id {
            if let Some(slot) = self
                .pending
                .iter_mut()
                .rev()
                .find(|(pid, _, _)| pid.as_ref().map(|s| s == target_id).unwrap_or(false))
            {
                slot.2 = input;
                return;
            }
        } else {
            // Null id: fall back to the most recent pending call without an id.
            // This matches v1's behavior of attaching deferred rawInput to the
            // last tool call even when the runtime didn't tag it.
            if let Some(slot) = self
                .pending
                .iter_mut()
                .rev()
                .find(|(pid, _, _)| pid.is_none())
            {
                slot.2 = input;
                return;
            }
        }
        debug!(
            ?id,
            "acp tool_call_update has no matching pending call — dropped"
        );
    }

    /// Return all pending calls in insertion order and reset the buffer.
    pub fn drain(&mut self) -> Vec<(Option<String>, String, Value)> {
        std::mem::take(&mut self.pending)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.len()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn json_rpc_request(id: u64, method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("json_rpc_request serialization should not fail")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Parse: responses ──────────────────────────────────────────────────

    #[test]
    fn parse_initialize_response() {
        let line =
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}"#;
        assert!(matches!(parse_line(line), AcpParsed::InitializeResponse));
    }

    #[test]
    fn parse_session_new_response_with_session_id() {
        let line = r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-abc"}}"#;
        match parse_line(line) {
            AcpParsed::SessionResponse {
                session_id: Some(id),
            } => assert_eq!(id, "sess-abc"),
            other => panic!("expected SessionResponse with id, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_load_response_omitting_session_id() {
        // kimi's session/load response: empty result, session id not echoed.
        let line = r#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
        match parse_line(line) {
            AcpParsed::SessionResponse { session_id: None } => {}
            other => panic!("expected SessionResponse with None, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_prompt_response() {
        // id 3 with a session id attached.
        let line =
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","sessionId":"s1"}}"#;
        match parse_line(line) {
            AcpParsed::PromptResponse {
                session_id: Some(id),
            } => assert_eq!(id, "s1"),
            other => panic!("expected PromptResponse, got {other:?}"),
        }

        // id 17 — follow-up prompt — parses as PromptResponse too.
        let line17 = r#"{"jsonrpc":"2.0","id":17,"result":{}}"#;
        match parse_line(line17) {
            AcpParsed::PromptResponse { session_id: None } => {}
            other => panic!("expected PromptResponse for id 17, got {other:?}"),
        }
    }

    // ── Parse: session/update notifications ──────────────────────────────

    #[test]
    fn parse_session_update_with_text() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"agentMessageChunk","chunk":"hello"}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::Text { text } => assert_eq!(text, "hello"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_text_snake_case_and_nested_content() {
        // kimi shape: snake_case kind via `sessionUpdate`, nested `content.text`.
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi there"}}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::Text { text } => assert_eq!(text, "hi there"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_thinking() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"agentThoughtChunk","chunk":"reasoning..."}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::Thinking { text } => assert_eq!(text, "reasoning..."),
                    other => panic!("expected Thinking, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_tool_call_and_tool_call_update() {
        // Back-to-back: first a tool_call with empty args, then a tool_call_update
        // with the real rawInput. Each parses independently; merging is the
        // transport's job via ToolCallAccumulator.
        let call = r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCall","toolCallId":"call-1","toolName":"mcp__chat__send_message","args":{}}}}"##;
        let update = r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCallUpdate","toolCallId":"call-1","rawInput":{"target":"#all","content":"hi"}}}}"##;

        match parse_line(call) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolCall { id, name, input } => {
                        assert_eq!(id.as_deref(), Some("call-1"));
                        assert_eq!(name, "send_message");
                        assert!(input.is_object());
                    }
                    other => panic!("expected ToolCall, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }

        match parse_line(update) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolCallUpdate { id, input } => {
                        assert_eq!(id.as_deref(), Some("call-1"));
                        assert_eq!(input["target"], "#all");
                        assert_eq!(input["content"], "hi");
                    }
                    other => panic!("expected ToolCallUpdate, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_tool_call_raw_input_initial() {
        // Some runtimes populate rawInput (not args) in the initial tool_call.
        let line = r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCall","toolCallId":"call-2","toolName":"mcp__chat__send_message","rawInput":{"target":"#general","content":"hello"},"status":"pending"}}}"##;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolCall { input, .. } => {
                        assert_eq!(input["target"], "#general");
                        assert_eq!(input["content"], "hello");
                    }
                    other => panic!("expected ToolCall, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_tool_result() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCallUpdate","toolCallId":"call-1","content":"Message sent successfully"}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolResult { content } => {
                        assert_eq!(content, "Message sent successfully");
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_structured_tool_result_kimi_shape() {
        // kimi nested: content array where each item is {content: {text, type}, type}.
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"c1","content":[{"content":{"type":"text","text":"line A"},"type":"content"},{"content":{"type":"text","text":"line B"},"type":"content"}]}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolResult { content } => {
                        assert_eq!(content, "line A\nline B");
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_with_structured_tool_result_flat_shape() {
        // flat: content array where each item is {type:"text", text:"..."}.
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCallUpdate","toolCallId":"c1","content":[{"type":"text","text":"alpha"},{"type":"text","text":"beta"}]}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolResult { content } => {
                        assert_eq!(content, "alpha\nbeta");
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_tool_call_update_with_both_raw_input_and_content() {
        // A single frame carrying deferred args AND a tool result.
        let line = r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCallUpdate","toolCallId":"call-1","rawInput":{"file_path":"/tmp/x"},"content":"File written successfully","status":"completed"}}}"##;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 3);
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => assert_eq!(session_id, "s1"),
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                assert!(matches!(items[1], AcpUpdateItem::ToolCallUpdate { .. }));
                match &items[2] {
                    AcpUpdateItem::ToolResult { content } => {
                        assert_eq!(content, "File written successfully");
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_update_ignored_kinds_yield_only_session_init() {
        // Informational kinds — parser emits SessionInit at [0] but no body items.
        for kind in [
            "userMessageChunk",
            "user_message_chunk",
            "plan",
            "availableCommandsUpdate",
            "currentModeUpdate",
            "configOptionUpdate",
            "sessionInfoUpdate",
        ] {
            let line = format!(
                r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"kind":"{kind}"}}}}}}"#
            );
            match parse_line(&line) {
                AcpParsed::SessionUpdate { items } => {
                    assert_eq!(
                        items.len(),
                        1,
                        "kind {kind} should yield only the prepended SessionInit"
                    );
                    match &items[0] {
                        AcpUpdateItem::SessionInit { session_id } => {
                            assert_eq!(session_id, "s1", "kind {kind} session_id mismatch")
                        }
                        other => panic!("expected SessionInit at [0] for kind {kind}, got {other:?}"),
                    }
                }
                other => panic!("expected SessionUpdate for kind {kind}, got {other:?}"),
            }
        }
    }

    #[test]
    fn session_update_notification_prepends_session_init() {
        // Per ACP spec, session/update notifications always carry params.sessionId.
        // The parser must emit AcpUpdateItem::SessionInit at position 0 so
        // multi-session drivers can route deterministically by the first item.
        let line = r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess_expected","update":{"kind":"toolCall","toolCallId":"call-xyz","toolName":"send_message","rawInput":{"target":"#general","content":"hi"}}}}"##;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert_eq!(items.len(), 2, "expected SessionInit + body item");
                match &items[0] {
                    AcpUpdateItem::SessionInit { session_id } => {
                        assert_eq!(session_id, "sess_expected");
                    }
                    other => panic!("expected SessionInit at [0], got {other:?}"),
                }
                match &items[1] {
                    AcpUpdateItem::ToolCall { id, name, input } => {
                        assert_eq!(id.as_deref(), Some("call-xyz"));
                        assert_eq!(name, "send_message");
                        assert_eq!(input["target"], "#general");
                        assert_eq!(input["content"], "hi");
                    }
                    other => panic!("expected ToolCall at [1], got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn session_update_missing_session_id_emits_no_session_init() {
        // Spec requires params.sessionId on every session/update. When absent,
        // we treat it as a malformed frame: emit the body items without a
        // prepended SessionInit and warn! (see parse_notification doc
        // comment). Drivers without a SessionInit at items[0] fall back to
        // their pick_session heuristics — emitting an empty-string
        // SessionInit would instead route the update to a nonexistent
        // session, breaking both Kimi (inserts "" as a session_id key) and
        // OpenCode (drops the update entirely).
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"kind":"agentMessageChunk","chunk":"hello"}}}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert!(
                    items
                        .iter()
                        .all(|i| !matches!(i, AcpUpdateItem::SessionInit { .. })),
                    "missing sessionId must not produce any SessionInit: {items:?}"
                );
                assert_eq!(items.len(), 1, "expected body item only: {items:?}");
                match &items[0] {
                    AcpUpdateItem::Text { text } => assert_eq!(text, "hello"),
                    other => panic!("expected Text at [0], got {other:?}"),
                }
            }
            other => panic!("expected SessionUpdate, got {other:?}"),
        }
    }

    #[test]
    fn session_update_missing_params_entirely_returns_empty_items() {
        // A session/update frame with no `params` key at all is fully
        // malformed. Rather than emitting an empty-string SessionInit
        // (which would route to a nonexistent session and cause Kimi /
        // OpenCode to misbehave), parse_notification returns an empty
        // items vec so drivers see a no-op.
        let line = r#"{"jsonrpc":"2.0","method":"session/update"}"#;
        match parse_line(line) {
            AcpParsed::SessionUpdate { items } => {
                assert!(
                    items.is_empty(),
                    "missing params must yield empty items vec: {items:?}"
                );
            }
            other => panic!("expected SessionUpdate with empty items, got {other:?}"),
        }
    }

    // Documented placeholder: TurnEnd is exposed on AcpUpdateItem so future
    // runtimes that inline turn-end into session/update can emit it without
    // requiring a parser change. Verify the variant constructs cleanly.
    #[test]
    fn acp_update_item_turn_end_variant_exists() {
        let t = AcpUpdateItem::TurnEnd;
        // Debug formatting must succeed; used in trace!/debug! logs.
        let _ = format!("{t:?}");
    }

    // ── Parse: permission requests ───────────────────────────────────────

    #[test]
    fn parse_permission_request() {
        let line = r#"{"jsonrpc":"2.0","id":42,"method":"session/request_permission","params":{"toolCall":{"title":"send_message: {target: #all}"},"options":[{"kind":"allow_always","optionId":"approve"}]}}"#;
        match parse_line(line) {
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                assert_eq!(request_id, 42);
                assert_eq!(tool_name.as_deref(), Some("send_message"));
                assert_eq!(options.len(), 1);
                assert_eq!(options[0].kind, "allow_always");
                assert_eq!(options[0].option_id, "approve");
            }
            other => panic!("expected PermissionRequested, got {other:?}"),
        }
    }

    #[test]
    fn parse_permission_request_with_bare_title() {
        // Title without ':' — tool name is the whole title, trimmed.
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"toolCall":{"title":"Write"}}}"#;
        match parse_line(line) {
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                assert_eq!(request_id, 7);
                assert_eq!(tool_name.as_deref(), Some("Write"));
                // No options array in this request — defaults to empty.
                assert!(options.is_empty());
            }
            other => panic!("expected PermissionRequested, got {other:?}"),
        }
    }

    // ── Parse: errors + unknown ──────────────────────────────────────────

    #[test]
    fn parse_error_response() {
        let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad"}}"#;
        match parse_line(line) {
            AcpParsed::Error { message } => assert_eq!(message, "bad"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_response_missing_message_falls_back() {
        let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600}}"#;
        match parse_line(line) {
            AcpParsed::Error { message } => assert_eq!(message, "unknown ACP error"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_line_not_json() {
        assert!(matches!(parse_line(""), AcpParsed::Unknown));
        assert!(matches!(parse_line("not json"), AcpParsed::Unknown));
        assert!(matches!(parse_line("{"), AcpParsed::Unknown));
    }

    #[test]
    fn parse_unknown_line_valid_json_but_not_acp() {
        // Missing both "method" and "id" — not a recognizable frame.
        assert!(matches!(
            parse_line(r#"{"hello":"world"}"#),
            AcpParsed::Unknown
        ));
    }

    #[test]
    fn parse_unknown_notification_method_returns_unknown() {
        // A non-"session/update" notification is Unknown, not empty SessionUpdate.
        let line = r#"{"jsonrpc":"2.0","method":"some/other","params":{}}"#;
        assert!(matches!(parse_line(line), AcpParsed::Unknown));
    }

    // ── Request builders ─────────────────────────────────────────────────

    #[test]
    fn build_initialize_request_has_expected_shape() {
        let raw = build_initialize_request(1);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["params"]["protocolVersion"], 1);
        assert_eq!(v["params"]["clientInfo"]["name"], "chorus");
        assert!(v["params"]["clientCapabilities"].is_object());
    }

    #[test]
    fn build_session_new_request_passes_through_params() {
        let params = json!({ "workspaceDir": "/tmp/wd" });
        let raw = build_session_new_request(2, params);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "session/new");
        assert_eq!(v["id"], 2);
        assert_eq!(v["params"]["workspaceDir"], "/tmp/wd");
    }

    #[test]
    fn build_session_load_request_splices_session_id() {
        let raw = build_session_load_request(2, "sess-xyz", json!({ "workspaceDir": "/wd" }));
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "session/load");
        assert_eq!(v["params"]["sessionId"], "sess-xyz");
        assert_eq!(v["params"]["workspaceDir"], "/wd");
    }

    #[test]
    fn build_session_prompt_request_without_session_id() {
        let raw = build_session_prompt_request(3, "", "hello");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "session/prompt");
        assert_eq!(v["params"]["prompt"][0]["text"], "hello");
        assert!(v["params"].get("sessionId").is_none());
    }

    #[test]
    fn build_session_prompt_request_with_session_id() {
        let raw = build_session_prompt_request(3, "sess-1", "hi");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["params"]["sessionId"], "sess-1");
        assert_eq!(v["params"]["prompt"][0]["text"], "hi");
    }

    #[test]
    fn build_permission_approval_response_approved() {
        let raw = build_permission_approval_response(42, true);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["id"], 42);
        assert_eq!(v["result"]["outcome"]["outcome"], "selected");
        assert_eq!(v["result"]["outcome"]["optionId"], "approve");
    }

    #[test]
    fn build_permission_approval_response_denied() {
        let raw = build_permission_approval_response(42, false);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["id"], 42);
        assert_eq!(v["result"]["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn build_permission_response_raw_uses_caller_option_id() {
        let raw = build_permission_response_raw(99, "allow_always_option");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["id"], 99);
        assert_eq!(v["result"]["outcome"]["optionId"], "allow_always_option");
    }

    // ── pick_best_option_id ──────────────────────────────────────────────

    #[test]
    fn pick_best_option_prefers_allow_always() {
        let options = vec![
            PermissionOption {
                kind: "allow_once".into(),
                option_id: "once-id".into(),
            },
            PermissionOption {
                kind: "allow_always".into(),
                option_id: "always-id".into(),
            },
        ];
        assert_eq!(pick_best_option_id(&options), "always-id");
    }

    #[test]
    fn pick_best_option_falls_back_to_allow_once() {
        let options = vec![
            PermissionOption {
                kind: "deny".into(),
                option_id: "deny-id".into(),
            },
            PermissionOption {
                kind: "allow_once".into(),
                option_id: "once-id".into(),
            },
        ];
        assert_eq!(pick_best_option_id(&options), "once-id");
    }

    #[test]
    fn pick_best_option_falls_back_to_first() {
        let options = vec![PermissionOption {
            kind: "custom".into(),
            option_id: "custom-id".into(),
        }];
        assert_eq!(pick_best_option_id(&options), "custom-id");
    }

    #[test]
    fn pick_best_option_empty_returns_approve() {
        // Some runtimes (kimi) omit options entirely — fallback to "approve".
        assert_eq!(pick_best_option_id(&[]), "approve");
    }

    #[test]
    fn parse_permission_request_with_multiple_options() {
        // claude-agent-acp sends both allow_once and allow_always with
        // runtime-generated optionIds — we must pick the right one.
        let line = r#"{"jsonrpc":"2.0","id":10,"method":"session/request_permission","params":{"toolCall":{"title":"check_messages"},"options":[{"kind":"allow_once","optionId":"oid-once"},{"kind":"allow_always","optionId":"oid-always"}]}}"#;
        match parse_line(line) {
            AcpParsed::PermissionRequested { options, .. } => {
                assert_eq!(options.len(), 2);
                assert_eq!(pick_best_option_id(&options), "oid-always");
            }
            other => panic!("expected PermissionRequested, got {other:?}"),
        }
    }

    // ── ToolCallAccumulator ──────────────────────────────────────────────

    #[test]
    fn tool_call_accumulator_merges_update_by_id() {
        let mut acc = ToolCallAccumulator::new();
        acc.record_call(
            Some("c1".to_string()),
            "send_message".to_string(),
            json!({}),
        );
        acc.merge_update(
            Some("c1".to_string()),
            json!({"target": "#all", "content": "hi"}),
        );
        let drained = acc.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0.as_deref(), Some("c1"));
        assert_eq!(drained[0].1, "send_message");
        assert_eq!(drained[0].2["target"], "#all");
        assert_eq!(drained[0].2["content"], "hi");
    }

    #[test]
    fn tool_call_accumulator_drops_orphan_update() {
        let mut acc = ToolCallAccumulator::new();
        // No pending calls — orphan update is dropped, accumulator stays empty.
        acc.merge_update(Some("nonexistent".to_string()), json!({"x": 1}));
        assert_eq!(acc.len(), 0);

        // With an unrelated pending call, the orphan still doesn't attach.
        acc.record_call(Some("a".to_string()), "foo".to_string(), json!({}));
        acc.merge_update(Some("b".to_string()), json!({"x": 1}));
        let drained = acc.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0.as_deref(), Some("a"));
        // Unchanged input — merge did not attach.
        assert_eq!(drained[0].2, json!({}));
    }

    #[test]
    fn tool_call_accumulator_drains_in_order() {
        let mut acc = ToolCallAccumulator::new();
        acc.record_call(
            Some("c1".to_string()),
            "tool_a".to_string(),
            json!({"a": 1}),
        );
        acc.record_call(
            Some("c2".to_string()),
            "tool_b".to_string(),
            json!({"b": 2}),
        );
        acc.record_call(None, "tool_c".to_string(), json!({"c": 3}));
        let drained = acc.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].1, "tool_a");
        assert_eq!(drained[1].1, "tool_b");
        assert_eq!(drained[2].1, "tool_c");
        // Drain empties the accumulator.
        assert_eq!(acc.len(), 0);
    }

    #[test]
    fn tool_call_accumulator_merge_null_id_targets_last_null_id_call() {
        // A tool_call with no id followed by a tool_call_update with no id:
        // the update should attach to the most recent un-id'd pending call.
        let mut acc = ToolCallAccumulator::new();
        acc.record_call(None, "send_message".to_string(), json!({}));
        acc.merge_update(None, json!({"text": "from null-id update"}));
        let drained = acc.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].2["text"], "from null-id update");
    }
}
