//! Stateless Claude headless mode helpers.
//!
//! Pure parsing and encoding for the Claude headless CLI streaming protocol.
//! No process lifecycle, no channels, no agent state — only data
//! transformation.
//!
//! The headless CLI emits one JSON object per line on stdout. Each line has a
//! top-level `"type"` field that routes to one of the [`HeadlessEvent`]
//! variants. Input messages are written as single JSON lines to stdin.

use serde_json::Value;
use tracing::warn;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A parsed event from one line of Claude headless stdout.
#[derive(Debug, Clone)]
pub enum HeadlessEvent {
    /// First line: system init with session id.
    SystemInit { session_id: String },
    /// API retry notification.
    ApiRetry { attempt: u32, error: String },
    /// Thinking content delta.
    ThinkingDelta { text: String },
    /// Text content delta.
    TextDelta { text: String },
    /// Tool use block started.
    ToolUseStart {
        index: u32,
        id: String,
        name: String,
    },
    /// Partial JSON input for a tool use block.
    InputJsonDelta { index: u32, partial_json: String },
    /// Tool use block stopped.
    ToolUseStop { index: u32 },
    /// Generic content block stop (text/thinking blocks).
    ContentBlockStop { index: u32 },
    /// Turn result (final event of a turn).
    TurnResult {
        session_id: String,
        result: String,
        is_error: bool,
        /// Why the turn ended: `"end_turn"`, `"tool_use"`, `"max_tokens"`, etc.
        stop_reason: String,
        /// Result status: `"success"` or `"error"`.
        subtype: String,
    },
    /// Unrecognized or irrelevant line.
    Unknown,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse one stdout JSONL line into a [`HeadlessEvent`].
pub fn parse_line(line: &str) -> HeadlessEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return HeadlessEvent::Unknown;
    }

    let v: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return HeadlessEvent::Unknown,
    };

    match v.get("type").and_then(|t| t.as_str()) {
        Some("system") => parse_system(&v),
        Some("stream_event") => parse_stream_event(&v),
        // "assistant" and "user" are partial-message echoes emitted by
        // --include-partial-messages. We don't need them.
        Some("assistant" | "user") => HeadlessEvent::Unknown,
        Some("result") => parse_result(&v),
        Some(other) => {
            warn!("claude headless: unknown event type: {other}");
            HeadlessEvent::Unknown
        }
        None => {
            warn!("claude headless: missing type field");
            HeadlessEvent::Unknown
        }
    }
}

fn parse_system(v: &Value) -> HeadlessEvent {
    match v.get("subtype").and_then(|s| s.as_str()) {
        Some("init") => {
            let session_id = v
                .get("session_id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::SystemInit { session_id }
        }
        Some("api_retry") => {
            let attempt = v.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0) as u32;
            let error = v
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::ApiRetry { attempt, error }
        }
        // hook_started / hook_response are emitted during session
        // initialization hooks (e.g. superpowers plugin). Safe to ignore.
        // "status" is emitted regularly by claude-code and is not actionable.
        Some("hook_started" | "hook_response" | "status") => HeadlessEvent::Unknown,
        Some(other) => {
            warn!("claude headless: unknown system subtype: {other}");
            HeadlessEvent::Unknown
        }
        None => {
            warn!("claude headless: system event missing subtype");
            HeadlessEvent::Unknown
        }
    }
}

fn parse_stream_event(v: &Value) -> HeadlessEvent {
    let event = match v.get("event") {
        Some(e) => e,
        None => return HeadlessEvent::Unknown,
    };

    match event.get("type").and_then(|t| t.as_str()) {
        Some("content_block_start") => parse_content_block_start(event),
        Some("content_block_delta") => parse_content_block_delta(event),
        Some("content_block_stop") => {
            let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            HeadlessEvent::ContentBlockStop { index }
        }
        Some("message_start" | "message_delta" | "message_stop") => HeadlessEvent::Unknown,
        Some(other) => {
            warn!("claude headless: unknown stream event type: {other}");
            HeadlessEvent::Unknown
        }
        None => HeadlessEvent::Unknown,
    }
}

fn parse_content_block_start(event: &Value) -> HeadlessEvent {
    let cb = match event.get("content_block") {
        Some(cb) => cb,
        None => return HeadlessEvent::Unknown,
    };

    match cb.get("type").and_then(|t| t.as_str()) {
        Some("tool_use") => {
            let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let id = cb
                .get("id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let name = cb
                .get("name")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::ToolUseStart { index, id, name }
        }
        _ => HeadlessEvent::Unknown,
    }
}

fn parse_content_block_delta(event: &Value) -> HeadlessEvent {
    let delta = match event.get("delta") {
        Some(d) => d,
        None => return HeadlessEvent::Unknown,
    };

    match delta.get("type").and_then(|t| t.as_str()) {
        Some("thinking_delta") => {
            let text = delta
                .get("thinking")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::ThinkingDelta { text }
        }
        Some("text_delta") => {
            let text = delta
                .get("text")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::TextDelta { text }
        }
        Some("input_json_delta") => {
            let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let partial_json = delta
                .get("partial_json")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            HeadlessEvent::InputJsonDelta {
                index,
                partial_json,
            }
        }
        _ => HeadlessEvent::Unknown,
    }
}

fn parse_result(v: &Value) -> HeadlessEvent {
    let session_id = v
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let result = v
        .get("result")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
    let stop_reason = v
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let subtype = v
        .get("subtype")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    HeadlessEvent::TurnResult {
        session_id,
        result,
        is_error,
        stop_reason,
        subtype,
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Build a user message JSON string for stdin. Does NOT include a trailing
/// newline — the caller is responsible for appending `\n`.
pub fn build_user_message(text: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": text,
        }
    }))
    .expect("build_user_message serialization should not fail")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_system_init() {
        let line = r#"{"type":"system","subtype":"init","session_id":"f574bca8-1234-5678-abcd-1234567890ab","tools":["Bash","Edit","Read"],"mcp_servers":[],"model":"claude-sonnet-4-6"}"#;
        match parse_line(line) {
            HeadlessEvent::SystemInit { session_id } => {
                assert_eq!(session_id, "f574bca8-1234-5678-abcd-1234567890ab");
            }
            other => panic!("expected SystemInit, got {other:?}"),
        }
    }

    #[test]
    fn parse_api_retry() {
        let line = r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":3,"retry_delay_ms":1000,"error":"rate_limit"}"#;
        match parse_line(line) {
            HeadlessEvent::ApiRetry { attempt, error } => {
                assert_eq!(attempt, 1);
                assert_eq!(error, "rate_limit");
            }
            other => panic!("expected ApiRetry, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}}"#;
        match parse_line(line) {
            HeadlessEvent::ThinkingDelta { text } => {
                assert_eq!(text, "Let me think...");
            }
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_text_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello!"}}}"#;
        match parse_line(line) {
            HeadlessEvent::TextDelta { text } => {
                assert_eq!(text, "Hello!");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_start() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_abc123","name":"Read","input":{}}}}"#;
        match parse_line(line) {
            HeadlessEvent::ToolUseStart { index, id, name } => {
                assert_eq!(index, 2);
                assert_eq!(id, "toolu_abc123");
                assert_eq!(name, "Read");
            }
            other => panic!("expected ToolUseStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"file\""}}}"#;
        match parse_line(line) {
            HeadlessEvent::InputJsonDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 2);
                assert_eq!(partial_json, r#"{"file""#);
            }
            other => panic!("expected InputJsonDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_content_block_stop() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#;
        match parse_line(line) {
            HeadlessEvent::ContentBlockStop { index } => {
                assert_eq!(index, 1);
            }
            other => panic!("expected ContentBlockStop, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_result_success() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello!","stop_reason":"end_turn","session_id":"abc123","duration_ms":1851,"total_cost_usd":0.026}"#;
        match parse_line(line) {
            HeadlessEvent::TurnResult {
                session_id,
                result,
                is_error,
                stop_reason,
                subtype,
            } => {
                assert_eq!(session_id, "abc123");
                assert_eq!(result, "Hello!");
                assert!(!is_error);
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(subtype, "success");
            }
            other => panic!("expected TurnResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_result_error() {
        let line = r#"{"type":"result","subtype":"error","is_error":true,"result":"Something went wrong","stop_reason":"error","session_id":"err456"}"#;
        match parse_line(line) {
            HeadlessEvent::TurnResult {
                session_id,
                result,
                is_error,
                stop_reason,
                subtype,
            } => {
                assert_eq!(session_id, "err456");
                assert_eq!(result, "Something went wrong");
                assert!(is_error);
                assert_eq!(stop_reason, "error");
                assert_eq!(subtype, "error");
            }
            other => panic!("expected TurnResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_type() {
        let line = r#"{"type":"foobar","data":"something"}"#;
        assert!(matches!(parse_line(line), HeadlessEvent::Unknown));
    }

    #[test]
    fn parse_malformed_json() {
        let line = "this is not json {{{";
        assert!(matches!(parse_line(line), HeadlessEvent::Unknown));
    }

    #[test]
    fn parse_empty_line() {
        assert!(matches!(parse_line(""), HeadlessEvent::Unknown));
        assert!(matches!(parse_line("  "), HeadlessEvent::Unknown));
    }

    #[test]
    fn parse_assistant_message_ignored() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]},"session_id":"abc123"}"#;
        assert!(matches!(parse_line(line), HeadlessEvent::Unknown));
    }

    #[test]
    fn parse_message_start_ignored() {
        let line = r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_123","role":"assistant"}}}"#;
        assert!(matches!(parse_line(line), HeadlessEvent::Unknown));
    }

    #[test]
    fn build_user_message_simple() {
        let msg = build_user_message("Hello world");
        let v: Value = serde_json::from_str(&msg).expect("valid JSON");
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "Hello world");
        // No trailing newline
        assert!(!msg.ends_with('\n'));
    }

    #[test]
    fn build_user_message_with_special_chars() {
        let msg = build_user_message("He said \"hello\"\nNew line");
        let v: Value = serde_json::from_str(&msg).expect("valid JSON");
        assert_eq!(v["message"]["content"], "He said \"hello\"\nNew line");
    }

    #[test]
    fn parse_full_turn_sequence() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"sess-001","tools":["Bash"],"mcp_servers":[],"model":"claude-sonnet-4-6"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Thinking..."}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi there!"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hi there!","stop_reason":"end_turn","session_id":"sess-001","duration_ms":500,"total_cost_usd":0.01}"#,
        ];

        let events: Vec<_> = lines.iter().map(|l| parse_line(l)).collect();

        // SystemInit
        assert!(
            matches!(&events[0], HeadlessEvent::SystemInit { session_id } if session_id == "sess-001")
        );
        // thinking block start → Unknown (we skip non-tool_use starts)
        assert!(matches!(&events[1], HeadlessEvent::Unknown));
        // thinking delta
        assert!(
            matches!(&events[2], HeadlessEvent::ThinkingDelta { text } if text == "Thinking...")
        );
        // content block stop index 0
        assert!(matches!(
            &events[3],
            HeadlessEvent::ContentBlockStop { index: 0 }
        ));
        // text block start → Unknown
        assert!(matches!(&events[4], HeadlessEvent::Unknown));
        // text delta
        assert!(matches!(&events[5], HeadlessEvent::TextDelta { text } if text == "Hi there!"));
        // content block stop index 1
        assert!(matches!(
            &events[6],
            HeadlessEvent::ContentBlockStop { index: 1 }
        ));
        // result
        assert!(
            matches!(&events[7], HeadlessEvent::TurnResult { session_id, is_error, .. } if session_id == "sess-001" && !is_error)
        );
    }
}
