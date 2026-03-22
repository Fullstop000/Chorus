use std::process::{Child, Command, Stdio};

use super::{Driver, ParsedEvent, SpawnContext};
use crate::models::AgentConfig;

pub struct CodexDriver;

impl Driver for CodexDriver {
    fn id(&self) -> &str {
        "codex"
    }

    fn supports_stdin_notification(&self) -> bool {
        false
    }

    fn mcp_tool_prefix(&self) -> &str {
        "mcp_chat_"
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        // Ensure git repo exists (codex requires it)
        let git_dir = std::path::Path::new(&ctx.working_directory).join(".git");
        if !git_dir.exists() {
            Command::new("git")
                .args(["init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            let git_env = [
                ("GIT_AUTHOR_NAME", "slock"),
                ("GIT_AUTHOR_EMAIL", "slock@local"),
                ("GIT_COMMITTER_NAME", "slock"),
                ("GIT_COMMITTER_EMAIL", "slock@local"),
            ];

            // Stage all and commit
            Command::new("git")
                .args(["add", "-A"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            Command::new("git")
                .args(["commit", "--allow-empty", "-m", "init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .envs(git_env)
                .status()?;
        }

        let bridge_binary_json = serde_json::to_string(&ctx.bridge_binary)?;
        let bridge_args_json = serde_json::to_string(&vec![
            "bridge",
            "--agent-id",
            &ctx.agent_id,
            "--server-url",
            &ctx.server_url,
        ])?;

        let mut args = vec!["exec".to_string()];

        if let Some(ref session_id) = ctx.config.session_id {
            args.push("resume".to_string());
            args.push(session_id.clone());
        }

        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        args.push("--json".to_string());

        args.push("-c".to_string());
        args.push(format!("mcp_servers.chat.command={bridge_binary_json}"));
        args.push("-c".to_string());
        args.push(format!("mcp_servers.chat.args={bridge_args_json}"));
        args.push("-c".to_string());
        args.push("mcp_servers.chat.startup_timeout_sec=30".to_string());
        args.push("-c".to_string());
        args.push("mcp_servers.chat.tool_timeout_sec=120".to_string());
        args.push("-c".to_string());
        args.push("mcp_servers.chat.enabled=true".to_string());
        args.push("-c".to_string());
        args.push("mcp_servers.chat.required=true".to_string());

        if !ctx.config.model.is_empty() {
            args.push("-m".to_string());
            args.push(ctx.config.model.clone());
        }

        // Prompt is the last positional arg
        args.push(ctx.prompt.clone());

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        env_vars.insert("NO_COLOR".to_string(), "1".to_string());
        if let Some(ref extra) = ctx.config.env_vars {
            for (k, v) in extra {
                env_vars.insert(k.clone(), v.clone());
            }
        }

        let child = Command::new("codex")
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
            "thread.started" => {
                if let Some(tid) = event.get("thread_id").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::SessionInit {
                        session_id: tid.to_string(),
                    });
                }
            }
            "turn.started" => {
                events.push(ParsedEvent::Thinking {
                    text: String::new(),
                });
            }
            "item.started" | "item.updated" | "item.completed" => {
                if let Some(item) = event.get("item") {
                    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match item_type {
                        "reasoning" => {
                            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                events.push(ParsedEvent::Thinking {
                                    text: text.to_string(),
                                });
                            }
                        }
                        "agent_message" => {
                            if event_type == "item.completed" {
                                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                    events.push(ParsedEvent::Text {
                                        text: text.to_string(),
                                    });
                                }
                            }
                        }
                        "command_execution" => {
                            if event_type == "item.started" {
                                let command = item
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                events.push(ParsedEvent::ToolCall {
                                    name: "shell".to_string(),
                                    input: serde_json::json!({ "command": command }),
                                });
                            }
                        }
                        "file_change" => {
                            if event_type == "item.started" {
                                if let Some(changes) =
                                    item.get("changes").and_then(|v| v.as_array())
                                {
                                    for change in changes {
                                        let path = change
                                            .get("path")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let kind = change
                                            .get("kind")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        events.push(ParsedEvent::ToolCall {
                                            name: "file_change".to_string(),
                                            input: serde_json::json!({
                                                "path": path,
                                                "kind": kind
                                            }),
                                        });
                                    }
                                }
                            }
                        }
                        "mcp_tool_call" => {
                            if event_type == "item.started" {
                                let server =
                                    item.get("server").and_then(|v| v.as_str()).unwrap_or("");
                                let tool =
                                    item.get("tool").and_then(|v| v.as_str()).unwrap_or("mcp_tool");
                                let name = if server == "chat" {
                                    format!("mcp_chat_{tool}")
                                } else {
                                    tool.to_string()
                                };
                                let arguments = item
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                events.push(ParsedEvent::ToolCall {
                                    name,
                                    input: arguments,
                                });
                            }
                        }
                        "collab_tool_call" => {
                            if event_type == "item.started" {
                                events.push(ParsedEvent::ToolCall {
                                    name: "collab_tool_call".to_string(),
                                    input: serde_json::json!({}),
                                });
                            }
                        }
                        "todo_list" => {
                            if event_type == "item.started" || event_type == "item.updated" {
                                let title = item
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Planning\u{2026}");
                                events.push(ParsedEvent::Thinking {
                                    text: title.to_string(),
                                });
                            }
                        }
                        "web_search" => {
                            if event_type == "item.started" {
                                let query = item
                                    .get("query")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                events.push(ParsedEvent::ToolCall {
                                    name: "web_search".to_string(),
                                    input: serde_json::json!({ "query": query }),
                                });
                            }
                        }
                        "error" => {
                            if let Some(msg) = item.get("message").and_then(|v| v.as_str()) {
                                events.push(ParsedEvent::Error {
                                    message: msg.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "turn.completed" => {
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            "turn.failed" => {
                if let Some(msg) = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                {
                    events.push(ParsedEvent::Error {
                        message: msg.to_string(),
                    });
                }
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            "error" => {
                let msg = event
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                events.push(ParsedEvent::Error {
                    message: msg.to_string(),
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
        let identity = if !config.display_name.is_empty() {
            config.display_name.as_str()
        } else {
            config.name.as_str()
        };
        let role = config
            .description
            .as_deref()
            .unwrap_or("You are a helpful Chorus agent.");

        format!(
            concat!(
                "You are \"{identity}\", an AI agent in Chorus — a collaborative platform for human-AI collaboration.\n\n",
                "## Who you are\n\n",
                "{role}\n\n",
                "You are a restart-driven agent runtime. Each Codex run is short-lived: wake up, check for work, handle what is available, then exit so the server can start you again when needed.\n\n",
                "## Communication — MCP tools ONLY\n\n",
                "Use ONLY these chat tools for communication and coordination:\n\n",
                "1. **mcp_chat_receive_message** — Use `block=false` to check for newly available messages.\n",
                "2. **mcp_chat_send_message** — Send a message to a channel or DM.\n",
                "3. **mcp_chat_list_server** — List channels, agents, and humans.\n",
                "4. **mcp_chat_read_history** — Read past messages from a channel, DM, or thread.\n",
                "5. **mcp_chat_list_tasks** — View a channel's task board.\n",
                "6. **mcp_chat_create_tasks** — Create tasks on a channel's task board.\n",
                "7. **mcp_chat_claim_tasks** — Claim tasks by number.\n",
                "8. **mcp_chat_unclaim_task** — Release your claim on a task.\n",
                "9. **mcp_chat_update_task_status** — Change a task's status.\n",
                "10. **mcp_chat_upload_file** — Upload an image file to attach to a message.\n",
                "11. **mcp_chat_view_file** — Download an attached image by its attachment ID so you can inspect it when relevant.\n\n",
                "CRITICAL RULES:\n",
                "- Do NOT output chat text directly. ALL communication goes through `mcp_chat_send_message`.\n",
                "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.\n",
                "- Reuse the exact `target` from received messages when replying.\n",
                "- Read MEMORY.md for persistent context after you have received work to do.\n",
                "- Do NOT explore the filesystem looking for messaging scripts. The MCP tools are already available.\n\n",
                "## Startup sequence\n\n",
                "1. Read `MEMORY.md` in your cwd.\n",
                "2. Follow MEMORY.md to any other notes you need.\n",
                "3. Immediately call `mcp_chat_receive_message(block=false)`.\n",
                "4. If messages are returned, process them and reply with `mcp_chat_send_message`.\n",
                "5. While work is still in progress, you may call `mcp_chat_receive_message(block=false)` at natural checkpoints to catch newly arrived messages.\n",
                "6. When there are no messages waiting and no work left, exit. Do not wait indefinitely; the server will start you again when needed.\n\n",
                "## Messaging\n\n",
                "Messages you receive have a structured header such as `[target=#general msg=a1b2c3d4 time=...] @user: ...`.\n",
                "- Reuse the exact `target` when replying.\n",
                "- Thread targets use `#channel:shortid` or `dm:@peer:shortid`.\n",
                "- Use `mcp_chat_read_history` when you need more context.\n",
                "- Use `mcp_chat_list_server` if you are unsure which channel or DM is appropriate.\n\n",
                "## Attachments\n\n",
                "If a message includes relevant image attachments, inspect them with `mcp_chat_view_file` before replying.\n\n",
                "## Task boards\n\n",
                "Task status is `todo` -> `in_progress` -> `in_review` -> `done`.\n",
                "- You MUST claim a task before starting work on it.\n",
                "- When you finish a meaningful task, move it to `in_review` instead of jumping straight to `done`.\n",
                "- After approval, set the task to `done` if the reviewer does not do it.\n",
                "- Prefer independent subtasks when breaking work apart.\n\n",
                "## Communication style\n\n",
                "- Acknowledge tasks briefly and outline your plan.\n",
                "- Send short progress updates for multi-step work.\n",
                "- Do not interrupt ongoing conversations unless you are explicitly addressed.\n",
                "- Do not echo work performed by another agent.\n\n",
                "## Formatting\n\n",
                "- Never output raw HTML tags in your messages.\n",
                "- Write mentions and channels as plain text like `@alice` and `#general`.\n",
                "- Do NOT wrap mentions or channel references in backticks.\n",
                "- Wrap URLs next to non-ASCII punctuation in angle brackets or markdown links.\n\n",
                "## Workspace & Memory\n\n",
                "Your cwd is your persistent workspace. Update MEMORY.md and supporting notes proactively so you can recover cleanly across restarts."
            ),
            identity = identity,
            role = role,
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "mcp_chat_send_message" => "Sending message\u{2026}".to_string(),
            "mcp_chat_receive_message" => "Receiving messages\u{2026}".to_string(),
            "mcp_chat_upload_file" => "Uploading file\u{2026}".to_string(),
            "mcp_chat_view_file" => "Viewing file\u{2026}".to_string(),
            "mcp_chat_list_tasks" => "Listing tasks\u{2026}".to_string(),
            "mcp_chat_create_tasks" => "Creating tasks\u{2026}".to_string(),
            "mcp_chat_claim_tasks" => "Claiming tasks\u{2026}".to_string(),
            "mcp_chat_unclaim_task" => "Unclaiming task\u{2026}".to_string(),
            "mcp_chat_update_task_status" => "Updating task status\u{2026}".to_string(),
            "mcp_chat_list_server" => "Listing server\u{2026}".to_string(),
            "mcp_chat_read_history" => "Reading history\u{2026}".to_string(),
            n if n.starts_with("mcp_chat_") => {
                let op = n.trim_start_matches("mcp_chat_").replace('_', " ");
                format!("Using {op}\u{2026}")
            }
            "shell" | "command_execution" => "Running command\u{2026}".to_string(),
            "file_change" => "Editing file\u{2026}".to_string(),
            "file_read" => "Reading file\u{2026}".to_string(),
            "file_write" => "Writing file\u{2026}".to_string(),
            "web_search" => "Searching web\u{2026}".to_string(),
            "collab_tool_call" => "Collaborating\u{2026}".to_string(),
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
            "shell" | "command_execution" => {
                let cmd = str_field("command");
                if cmd.chars().count() > 100 {
                    let truncated: String = cmd.chars().take(100).collect();
                    format!("{truncated}\u{2026}")
                } else {
                    cmd
                }
            }
            "web_search" => str_field("query"),
            "mcp_chat_send_message" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "mcp_chat_read_history" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "mcp_chat_list_tasks" | "mcp_chat_create_tasks" => str_field("channel"),
            "mcp_chat_claim_tasks" => {
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
            "mcp_chat_unclaim_task" | "mcp_chat_update_task_status" => {
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
            "mcp_chat_upload_file" => str_field("file_path"),
            _ => String::new(),
        }
    }
}
