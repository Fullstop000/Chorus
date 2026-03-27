use std::io::Write as _;
use std::process::{Child, Command, Stdio};

use super::{Driver, ParsedEvent, SpawnContext};
use crate::agent::drivers::prompt::{build_base_system_prompt, PromptOptions};
use crate::store::agents::AgentConfig;

pub struct KimiDriver;

fn normalize_kimi_tool_name(name: &str) -> String {
    match name {
        "mcp__chat__send_message" => "send_message".to_string(),
        "mcp__chat__check_messages" => "check_messages".to_string(),
        "mcp__chat__wait_for_message" => "wait_for_message".to_string(),
        "mcp__chat__receive_message" => "receive_message".to_string(),
        "mcp__chat__upload_file" => "upload_file".to_string(),
        "mcp__chat__view_file" => "view_file".to_string(),
        "mcp__chat__list_tasks" => "list_tasks".to_string(),
        "mcp__chat__create_tasks" => "create_tasks".to_string(),
        "mcp__chat__claim_tasks" => "claim_tasks".to_string(),
        "mcp__chat__unclaim_task" => "unclaim_task".to_string(),
        "mcp__chat__update_task_status" => "update_task_status".to_string(),
        "mcp__chat__list_server" => "list_server".to_string(),
        "mcp__chat__read_history" => "read_history".to_string(),
        other => other.to_string(),
    }
}

impl Driver for KimiDriver {
    fn id(&self) -> &str {
        "kimi"
    }

    fn supports_stdin_notification(&self) -> bool {
        true
    }

    fn mcp_tool_prefix(&self) -> &str {
        ""
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": ctx.bridge_binary,
                    "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        let mcp_config_path =
            std::path::Path::new(&ctx.working_directory).join(".chorus-kimi-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;

        let session_id = ctx
            .config
            .session_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Kimi requires a prepared session id"))?;

        let mut args = vec![
            "--print".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--work-dir".to_string(),
            ctx.working_directory.clone(),
            "--session".to_string(),
            session_id.to_string(),
            "--mcp-config-file".to_string(),
            mcp_config_path.to_string_lossy().into_owned(),
        ];

        if !ctx.config.model.is_empty() {
            args.push("--model".to_string());
            args.push(ctx.config.model.clone());
        }

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        env_vars.insert("NO_COLOR".to_string(), "1".to_string());
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }

        let mut child = Command::new("kimi")
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&env_vars)
            .spawn()?;

        let stdin_msg = serde_json::json!({
            "role": "user",
            "content": &ctx.prompt,
        });

        if let Some(ref mut stdin) = child.stdin {
            let mut line = serde_json::to_string(&stdin_msg)?;
            line.push('\n');
            stdin.write_all(line.as_bytes())?;
        }

        Ok(child)
    }

    fn parse_line(&self, line: &str) -> Vec<ParsedEvent> {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let mut events = Vec::new();
        match event.get("role").and_then(|v| v.as_str()) {
            Some("assistant") => {
                let mut has_tool_calls = false;

                match event.get("content") {
                    Some(serde_json::Value::String(text)) => {
                        events.push(ParsedEvent::Text {
                            text: text.to_string(),
                        });
                    }
                    Some(serde_json::Value::Array(content)) => {
                        for block in content {
                            match block.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                        events.push(ParsedEvent::Text {
                                            text: text.to_string(),
                                        });
                                    }
                                }
                                Some("tool_use") => {
                                    has_tool_calls = true;
                                    let name = normalize_kimi_tool_name(
                                        block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown_tool"),
                                    );
                                    let input = block
                                        .get("input")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    events.push(ParsedEvent::ToolCall { name, input });
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }

                if let Some(tool_calls) = event.get("tool_calls").and_then(|v| v.as_array()) {
                    for tool_call in tool_calls {
                        has_tool_calls = true;
                        let name = normalize_kimi_tool_name(
                            tool_call
                                .get("function")
                                .and_then(|function| function.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown_tool"),
                        );
                        let input = tool_call
                            .get("function")
                            .and_then(|function| function.get("arguments"))
                            .and_then(|v| v.as_str())
                            .and_then(|value| serde_json::from_str(value).ok())
                            .unwrap_or(serde_json::Value::Null);
                        events.push(ParsedEvent::ToolCall { name, input });
                    }
                }

                if !has_tool_calls {
                    events.push(ParsedEvent::TurnEnd { session_id: None });
                }
            }
            Some("tool") => {}
            Some("error") => {
                let message = event
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown Kimi error");
                events.push(ParsedEvent::Error {
                    message: message.to_string(),
                });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, text: &str, _session_id: &str) -> Option<String> {
        let msg = serde_json::json!({
            "role": "user",
            "content": text,
        });
        Some(serde_json::to_string(&msg).unwrap_or_default())
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: "".to_string(),
                extra_critical_rules: vec![
                    "- Do NOT use bash/curl/sqlite to send or receive messages. The MCP tools handle everything.".to_string(),
                    "- Call `wait_for_message()` when you are idle so the agent stays in the receive loop.".to_string(),
                    "- After `wait_for_message()` or `check_messages()` returns a real user message, you must either send a reply or deliberately explain why no reply is needed before going idle again.".to_string(),
                    "- Direct messages and explicit @mentions are addressed to you. Do not silently consume them and return to waiting.".to_string(),
                    "- Never treat raw assistant stdout as a user-visible reply. Any reply meant for humans must be delivered with `send_message()`.".to_string(),
                ],
                post_startup_notes: vec![],
                include_stdin_notification_section: true,
                teams: config.teams.clone(),
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "send_message" => "Sending message…".to_string(),
            "check_messages" => "Checking messages…".to_string(),
            "wait_for_message" => "Waiting for messages…".to_string(),
            "receive_message" => "Receiving messages…".to_string(),
            "upload_file" => "Uploading file…".to_string(),
            "view_file" => "Viewing file…".to_string(),
            "list_tasks" => "Listing tasks…".to_string(),
            "create_tasks" => "Creating tasks…".to_string(),
            "claim_tasks" => "Claiming tasks…".to_string(),
            "unclaim_task" => "Unclaiming task…".to_string(),
            "update_task_status" => "Updating task status…".to_string(),
            "list_server" => "Listing server…".to_string(),
            "read_history" => "Reading history…".to_string(),
            n if n.starts_with("mcp__chat__") => {
                let op = normalize_kimi_tool_name(n).replace('_', " ");
                format!("Using {op}…")
            }
            other => {
                let truncated: String = other.chars().take(20).collect();
                format!("Using {truncated}…")
            }
        }
    }

    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String {
        if !input.is_object() {
            return String::new();
        }

        let str_field = |field: &str| -> String {
            input
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        match name {
            "Read" | "read_file" | "Write" | "write_file" | "Edit" | "edit_file" => {
                let p = str_field("file_path");
                if p.is_empty() {
                    str_field("path")
                } else {
                    p
                }
            }
            "Bash" | "bash" => {
                let cmd = str_field("command");
                if cmd.chars().count() > 100 {
                    let truncated: String = cmd.chars().take(100).collect();
                    format!("{truncated}…")
                } else {
                    cmd
                }
            }
            "Glob" | "glob" | "Grep" | "grep" => str_field("pattern"),
            "WebFetch" | "web_fetch" => str_field("url"),
            "send_message" | "mcp__chat__send_message" => {
                let target = str_field("target");
                let text = {
                    let content = str_field("content");
                    if content.is_empty() {
                        str_field("text")
                    } else {
                        content
                    }
                };
                if target.is_empty() {
                    text
                } else if text.is_empty() {
                    target
                } else {
                    format!("{target}: {text}")
                }
            }
            "read_history" | "mcp__chat__read_history" => str_field("channel"),
            "view_file" | "mcp__chat__view_file" => str_field("path"),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_parse_line_maps_documented_assistant_text_output() {
        let driver = KimiDriver;
        let events =
            driver.parse_line(r#"{"role":"assistant","content":"Hello! How can I help you?"}"#);

        assert!(matches!(
            &events[0],
            ParsedEvent::Text { text } if text == "Hello! How can I help you?"
        ));
        assert!(matches!(events[1], ParsedEvent::TurnEnd { .. }));
    }

    #[test]
    fn kimi_parse_line_maps_documented_tool_sequence() {
        let driver = KimiDriver;
        let events = driver.parse_line(
            r#"{"role":"assistant","content":[{"type":"tool_use","name":"check_messages","input":{"limit":5}}]}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::ToolCall { name, .. } if name == "check_messages"
        ));
    }

    #[test]
    fn kimi_parse_line_maps_top_level_tool_calls() {
        let driver = KimiDriver;
        let events = driver.parse_line(
            r#"{"role":"assistant","content":[{"type":"think","think":"..." }],"tool_calls":[{"type":"function","id":"tool_1","function":{"name":"send_message","arguments":"{\"target\":\"dm:@bytedance\",\"content\":\"TRACE-KIMI-123\"}"}}]}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::ToolCall { name, input }
                if name == "send_message" && input["target"] == "dm:@bytedance"
        ));
    }

    #[test]
    fn kimi_encode_stdin_message_uses_documented_message_shape() {
        let driver = KimiDriver;
        let encoded = driver
            .encode_stdin_message("Hello", "ignored")
            .expect("Kimi driver should support stdin messages");
        let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();

        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello");
    }

    #[test]
    fn kimi_summarize_tool_input_includes_read_history_channel() {
        let driver = KimiDriver;
        let input = serde_json::json!({ "channel": "#common-feature-squad", "limit": 20 });

        assert_eq!(
            driver.summarize_tool_input("read_history", &input),
            "#common-feature-squad"
        );
    }
}
