use std::io::Write as _;
use std::process::{Child, Command, Stdio};

use super::{command_exists, run_command, Driver, ParsedEvent, SpawnContext};
use crate::agent::drivers::prompt::{build_base_system_prompt, PromptOptions};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::{AgentConfig, AgentRuntime};

pub struct ClaudeDriver;

impl Driver for ClaudeDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Claude
    }

    fn supports_stdin_notification(&self) -> bool {
        true
    }

    fn mcp_tool_prefix(&self) -> &str {
        "mcp__chat__"
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        // Write MCP config file
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": ctx.bridge_binary,
                    "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        let mcp_config_path = std::path::Path::new(&ctx.working_directory).join(".chorus-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;

        let mut args = vec![
            "--allow-dangerously-skip-permissions".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--mcp-config".to_string(),
            mcp_config_path.to_string_lossy().into_owned(),
            "--disallowed-tools".to_string(),
            "EnterPlanMode,ExitPlanMode".to_string(),
            "--model".to_string(),
            if ctx.config.model.is_empty() {
                "sonnet".to_string()
            } else {
                ctx.config.model.clone()
            },
        ];

        if let Some(ref session_id) = ctx.config.session_id {
            args.push("--resume".to_string());
            args.push(session_id.clone());
        }

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }
        env_vars.remove("CLAUDECODE");

        let mut child = Command::new("claude")
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&env_vars)
            .spawn()?;

        // Send initial user message via stdin
        let stdin_msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": &ctx.prompt}]
            }
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

        match event.get("type").and_then(|v| v.as_str()) {
            Some("system") => {
                if event.get("subtype").and_then(|v| v.as_str()) == Some("init") {
                    if let Some(sid) = event.get("session_id").and_then(|v| v.as_str()) {
                        events.push(ParsedEvent::SessionInit {
                            session_id: sid.to_string(),
                        });
                    }
                }
            }
            Some("assistant") => {
                if let Some(content) = event
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|v| v.as_str()) {
                            Some("thinking") => {
                                if let Some(text) = block.get("thinking").and_then(|v| v.as_str()) {
                                    events.push(ParsedEvent::Thinking {
                                        text: text.to_string(),
                                    });
                                }
                            }
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                    events.push(ParsedEvent::Text {
                                        text: text.to_string(),
                                    });
                                }
                            }
                            Some("tool_use") => {
                                let name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown_tool")
                                    .to_string();
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
            }
            Some("result") => {
                let session_id = event
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                events.push(ParsedEvent::TurnEnd { session_id });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String> {
        let msg = if session_id.is_empty() {
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": text}]
                }
            })
        } else {
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": text}]
                },
                "session_id": session_id
            })
        };
        Some(serde_json::to_string(&msg).unwrap_or_default())
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: "mcp__chat__".to_string(),
                extra_critical_rules: vec![
                    "- Do NOT use bash/curl/sqlite to send or receive messages. The MCP tools handle everything.".to_string(),
                ],
                post_startup_notes: vec![],
                include_stdin_notification_section: true,
                teams: config.teams.clone(),
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "mcp__chat__send_message" => "Sending message\u{2026}".to_string(),
            "mcp__chat__check_messages" => "Checking messages\u{2026}".to_string(),
            "mcp__chat__wait_for_message" => "Waiting for messages\u{2026}".to_string(),
            "mcp__chat__receive_message" => "Receiving messages\u{2026}".to_string(),
            "mcp__chat__upload_file" => "Uploading file\u{2026}".to_string(),
            "mcp__chat__view_file" => "Viewing file\u{2026}".to_string(),
            "mcp__chat__list_tasks" => "Listing tasks\u{2026}".to_string(),
            "mcp__chat__create_tasks" => "Creating tasks\u{2026}".to_string(),
            "mcp__chat__claim_tasks" => "Claiming tasks\u{2026}".to_string(),
            "mcp__chat__unclaim_task" => "Unclaiming task\u{2026}".to_string(),
            "mcp__chat__update_task_status" => "Updating task status\u{2026}".to_string(),
            "mcp__chat__list_server" => "Listing server\u{2026}".to_string(),
            "mcp__chat__read_history" => "Reading history\u{2026}".to_string(),
            n if n.starts_with("mcp__chat__") => {
                let op = n.trim_start_matches("mcp__chat__").replace('_', " ");
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
                    format!("{truncated}\u{2026}")
                } else {
                    cmd
                }
            }
            "Glob" | "glob" | "Grep" | "grep" => str_field("pattern"),
            "WebFetch" | "web_fetch" => str_field("url"),
            "WebSearch" | "web_search" => str_field("query"),
            "mcp__chat__check_messages" | "mcp__chat__wait_for_message" => String::new(),
            "mcp__chat__send_message" => {
                let t = str_field("target");
                let target = if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                };
                let content = str_field("content");
                if content.is_empty() {
                    target
                } else {
                    let preview: String = content.chars().take(80).collect();
                    let preview = if content.chars().count() > 80 {
                        format!("{preview}\u{2026}")
                    } else {
                        preview
                    };
                    if target.is_empty() {
                        preview
                    } else {
                        format!("{target}: {preview}")
                    }
                }
            }
            "mcp__chat__read_history" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "mcp__chat__list_tasks" | "mcp__chat__create_tasks" => str_field("channel"),
            "mcp__chat__claim_tasks" => {
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
            "mcp__chat__unclaim_task" | "mcp__chat__update_task_status" => {
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
            "mcp__chat__upload_file" => str_field("file_path"),
            _ => String::new(),
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("claude") {
            return Ok(RuntimeStatus {
                runtime: self.id().to_string(),
                installed: false,
                auth_status: None,
            });
        }

        let auth_status = run_command("claude", &["auth", "status"])
            .ok()
            .and_then(|result| {
                if !result.success {
                    return Some(RuntimeAuthStatus::Unauthed);
                }
                let payload: serde_json::Value = serde_json::from_str(&result.stdout).ok()?;
                Some(if payload["loggedIn"].as_bool().unwrap_or(false) {
                    RuntimeAuthStatus::Authed
                } else {
                    RuntimeAuthStatus::Unauthed
                })
            })
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: self.id().to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }
}
