use std::process::{Child, Command, Stdio};

use super::prompt::{build_base_system_prompt, PromptOptions};
use super::{command_exists, run_command, Driver, ParsedEvent, SpawnContext};
use crate::agent::config::AgentConfig;
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::AgentRuntime;

pub struct OpencodeRawDriver;

fn parse_opencode_models(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

impl Driver for OpencodeRawDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Opencode
    }

    fn supports_stdin_notification(&self) -> bool {
        false
    }

    fn mcp_tool_prefix(&self) -> &str {
        "chat_"
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        // Write opencode.json MCP config in the working directory.
        let mcp_config = serde_json::json!({
            "mcp": {
                "chat": {
                    "type": "local",
                    "command": [&ctx.bridge_binary, "bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        // OpenCode reads opencode.json from the working directory automatically.
        let config_path = std::path::Path::new(&ctx.working_directory).join("opencode.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&mcp_config)?)?;

        let mut args = vec![
            "run".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--thinking".to_string(),
        ];

        if let Some(ref session_id) = ctx.config.session_id {
            args.push("--session".to_string());
            args.push(session_id.clone());
        }

        if !ctx.config.model.is_empty() {
            args.push("--model".to_string());
            args.push(ctx.config.model.clone());
        }

        if let Some(ref variant) = ctx.config.reasoning_effort {
            args.push("--variant".to_string());
            args.push(variant.clone());
        }

        // Positional prompt argument comes last.
        args.push(ctx.prompt.clone());

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        env_vars.insert("NO_COLOR".to_string(), "1".to_string());
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }

        let child = Command::new("opencode")
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&env_vars)
            .spawn()?;

        Ok(child)
    }

    fn parse_line(&self, line: &str) -> Vec<ParsedEvent> {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let mut events = Vec::new();
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "step_start" => {
                if let Some(session_id) = event.get("sessionID").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::SessionInit {
                        session_id: session_id.to_string(),
                    });
                }
                events.push(ParsedEvent::Thinking {
                    text: String::new(),
                });
            }
            "reasoning" => {
                let text = event
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                events.push(ParsedEvent::Thinking {
                    text: text.to_string(),
                });
            }
            "text" => {
                let text = event
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                events.push(ParsedEvent::Text {
                    text: text.to_string(),
                });
            }
            "tool_use" => {
                if let Some(part) = event.get("part") {
                    let state = part.get("state").and_then(|v| v.as_str()).unwrap_or("");
                    // Emit tool calls on pending/running states (start of tool use).
                    if state == "pending" || state == "running" {
                        let tool = part
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown_tool");
                        let input = part
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);

                        // OpenCode names MCP tools as {server}_{tool}.
                        // Tools from the "chat" server are prefixed chat_.
                        events.push(ParsedEvent::ToolCall {
                            name: tool.to_string(),
                            input,
                        });
                    }
                }
            }
            "step_finish" => {
                let session_id = event
                    .get("sessionID")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                events.push(ParsedEvent::TurnEnd { session_id });
            }
            "error" => {
                let message = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .or_else(|| event.get("message").and_then(|v| v.as_str()))
                    .unwrap_or("Unknown error");
                events.push(ParsedEvent::Error {
                    message: message.to_string(),
                });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, _text: &str, _session_id: &str) -> Option<String> {
        None
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: "chat_".to_string(),
                extra_critical_rules: vec![
                    "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".to_string(),
                ],
                post_startup_notes: vec![
                    "**IMPORTANT**: Complete your work and stop. The server will wake you when new work arrives.".to_string(),
                ],
                include_stdin_notification_section: false,
                teams: config.teams.clone(),
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "chat_send_message" => "Sending message\u{2026}".to_string(),
            "chat_check_messages" => "Checking messages\u{2026}".to_string(),
            "chat_upload_file" => "Uploading file\u{2026}".to_string(),
            "chat_view_file" => "Viewing file\u{2026}".to_string(),
            "chat_list_tasks" => "Listing tasks\u{2026}".to_string(),
            "chat_create_tasks" => "Creating tasks\u{2026}".to_string(),
            "chat_claim_tasks" => "Claiming tasks\u{2026}".to_string(),
            "chat_unclaim_task" => "Unclaiming task\u{2026}".to_string(),
            "chat_update_task_status" => "Updating task status\u{2026}".to_string(),
            "chat_list_server" => "Listing server\u{2026}".to_string(),
            "chat_read_history" => "Reading history\u{2026}".to_string(),
            n if n.starts_with("chat_") => {
                let op = n.trim_start_matches("chat_").replace('_', " ");
                format!("Using {op}\u{2026}")
            }
            other => {
                let truncated: String = other.chars().take(20).collect();
                format!("Using {truncated}\u{2026}")
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
            "file_read" | "file_write" | "file_change" => {
                let p = str_field("path");
                if p.is_empty() {
                    str_field("file_path")
                } else {
                    p
                }
            }
            "shell" | "bash" | "command_execution" => {
                let cmd = str_field("command");
                if cmd.chars().count() > 100 {
                    let truncated: String = cmd.chars().take(100).collect();
                    format!("{truncated}\u{2026}")
                } else {
                    cmd
                }
            }
            "web_search" => str_field("query"),
            "chat_check_messages" => String::new(),
            "chat_send_message" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "chat_read_history" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "chat_list_tasks" | "chat_create_tasks" => str_field("channel"),
            "chat_claim_tasks" => {
                let channel = str_field("channel");
                if channel.is_empty() {
                    return String::new();
                }
                let nums = input.get("task_numbers");
                let nums_str = match nums {
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
                        .map(|n| format!("#t{n}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    Some(v) => {
                        if let Some(n) = v.as_i64() {
                            format!("#t{n}")
                        } else {
                            format!("#t{v}")
                        }
                    }
                    None => return channel,
                };
                format!("{channel} {nums_str}")
            }
            "chat_unclaim_task" | "chat_update_task_status" => {
                let channel = str_field("channel");
                if channel.is_empty() {
                    return String::new();
                }
                let tn = input
                    .get("task_number")
                    .and_then(|v| v.as_i64())
                    .map(|n| format!("#t{n}"))
                    .unwrap_or_default();
                format!("{channel} {tn}")
            }
            "chat_upload_file" => str_field("file_path"),
            _ => String::new(),
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("opencode") {
            return Ok(RuntimeStatus {
                runtime: self.id().to_string(),
                installed: false,
                auth_status: None,
            });
        }

        // OpenCode is provider-agnostic; auth depends on the chosen provider's
        // API keys being present in the environment.  A simple version probe
        // confirms the binary works.
        let auth_status = run_command("opencode", &["--version"])
            .ok()
            .map(|result| {
                if result.success {
                    RuntimeAuthStatus::Authed
                } else {
                    RuntimeAuthStatus::Unauthed
                }
            })
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: self.id().to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        if !command_exists("opencode") {
            return Ok(Vec::new());
        }

        let result = run_command("opencode", &["models"])?;
        if !result.success {
            anyhow::bail!("failed to list opencode models: {}", result.stderr.trim());
        }

        Ok(parse_opencode_models(&result.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_opencode_models_skips_blank_lines() {
        assert_eq!(
            parse_opencode_models("\nopenai/gpt-5.4\n\nopenai/gpt-5.4-mini\n"),
            vec![
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string()
            ]
        );
    }

    #[test]
    fn opencode_parse_line_maps_text_event() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r#"{"type":"text","timestamp":1711929600000,"sessionID":"sess-1","part":{"type":"text","text":"Hello world"}}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::Text { text } if text == "Hello world"
        ));
    }

    #[test]
    fn opencode_parse_line_maps_tool_use_event() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r##"{"type":"tool_use","timestamp":1711929600000,"sessionID":"sess-1","part":{"tool":"chat_send_message","state":"running","input":{"target":"#all","content":"hi"}}}"##,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::ToolCall { name, input }
                if name == "chat_send_message" && input["target"] == "#all"
        ));
    }

    #[test]
    fn opencode_parse_line_ignores_completed_tool_state() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r#"{"type":"tool_use","timestamp":1711929600000,"sessionID":"sess-1","part":{"tool":"bash","state":"completed","input":{}}}"#,
        );

        assert!(events.is_empty());
    }

    #[test]
    fn opencode_parse_line_maps_step_finish_with_session_id() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r#"{"type":"step_finish","timestamp":1711929600000,"sessionID":"sess-42"}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::TurnEnd { session_id } if session_id.as_deref() == Some("sess-42")
        ));
    }

    #[test]
    fn opencode_parse_line_maps_error_event() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r#"{"type":"error","timestamp":1711929600000,"sessionID":"sess-1","error":{"message":"rate limited"}}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::Error { message } if message == "rate limited"
        ));
    }

    #[test]
    fn opencode_parse_line_maps_reasoning_event() {
        let driver = OpencodeRawDriver;
        let events = driver.parse_line(
            r#"{"type":"reasoning","timestamp":1711929600000,"sessionID":"sess-1","part":{"type":"reasoning","text":"Let me think..."}}"#,
        );

        assert!(matches!(
            &events[0],
            ParsedEvent::Thinking { text } if text == "Let me think..."
        ));
    }

    #[test]
    fn opencode_parse_line_step_start_emits_session_init() {
        let driver = OpencodeRawDriver;
        let events = driver
            .parse_line(r#"{"type":"step_start","timestamp":1711929600000,"sessionID":"sess-99"}"#);

        assert!(matches!(
            &events[0],
            ParsedEvent::SessionInit { session_id } if session_id == "sess-99"
        ));
    }

    #[test]
    fn parse_line_ignores_non_json() {
        let d = OpencodeRawDriver;
        assert!(d.parse_line("not json").is_empty());
        assert!(d.parse_line("").is_empty());
    }

    #[test]
    fn parse_line_step_start() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"step_start","sessionID":"sess-oc-1"}"#);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], ParsedEvent::SessionInit { session_id } if session_id == "sess-oc-1")
        );
        assert!(matches!(&events[1], ParsedEvent::Thinking { .. }));
    }

    #[test]
    fn parse_line_reasoning() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"reasoning","part":{"text":"pondering"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Thinking { text } if text == "pondering"));
    }

    #[test]
    fn parse_line_text() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"text","part":{"text":"done"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Text { text } if text == "done"));
    }

    #[test]
    fn parse_line_tool_use_pending() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"tool_use","part":{"state":"pending","tool":"chat_send_message","input":{}}}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::ToolCall { name, .. } if name == "chat_send_message")
        );
    }

    #[test]
    fn parse_line_tool_use_completed_ignored() {
        let d = OpencodeRawDriver;
        // only pending/running states emit ToolCall
        assert!(d.parse_line(r#"{"type":"tool_use","part":{"state":"completed","tool":"chat_send_message","input":{}}}"#).is_empty());
    }

    #[test]
    fn parse_line_step_finish_with_session() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"step_finish","sessionID":"sess-oc-2"}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::TurnEnd { session_id: Some(id) } if id == "sess-oc-2")
        );
    }

    #[test]
    fn parse_line_error_nested() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"error","error":{"message":"network failure"}}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::Error { message } if message == "network failure")
        );
    }

    #[test]
    fn parse_line_error_flat() {
        let d = OpencodeRawDriver;
        let events = d.parse_line(r#"{"type":"error","message":"timeout"}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Error { message } if message == "timeout"));
    }
}
