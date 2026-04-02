use std::process::{Child, Command, Stdio};

use super::{Driver, ParsedEvent, SpawnContext};
use crate::agent::config::AgentConfig;
use crate::agent::drivers::prompt::{build_base_system_prompt, PromptOptions};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::AgentRuntime;

pub struct StubDriver;

impl Driver for StubDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Stub
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
            std::path::Path::new(&ctx.working_directory).join(".chorus-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;

        let stub_binary = std::env::current_exe()?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("cannot find binary directory"))?
            .join("chorus-stub-agent");

        let delay_ms = std::env::var("STUB_DELAY_MS").unwrap_or_else(|_| "200".to_string());

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("STUB_DELAY_MS".to_string(), delay_ms);
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }

        let child = Command::new(&stub_binary)
            .args([
                "--mcp-config",
                &mcp_config_path.to_string_lossy(),
                "--prompt",
                &ctx.prompt,
            ])
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
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

        match event.get("type").and_then(|v| v.as_str()) {
            Some("session_init") => {
                if let Some(sid) = event.get("session_id").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::SessionInit {
                        session_id: sid.to_string(),
                    });
                }
            }
            Some("text") => {
                if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::Text {
                        text: text.to_string(),
                    });
                }
            }
            Some("tool_call") => {
                let name = event
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input = event
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                events.push(ParsedEvent::ToolCall { name, input });
            }
            Some("turn_end") => {
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            Some("error") => {
                let message = event
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                events.push(ParsedEvent::Error { message });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, text: &str, _session_id: &str) -> Option<String> {
        let msg = serde_json::json!({
            "type": "notification",
            "content": text,
        });
        Some(serde_json::to_string(&msg).unwrap_or_default())
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: String::new(),
                extra_critical_rules: vec![],
                post_startup_notes: vec![],
                include_stdin_notification_section: true,
                teams: config.teams.clone(),
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "send_message" => "Sending message\u{2026}".to_string(),
            "check_messages" => "Checking messages\u{2026}".to_string(),
            "wait_for_message" => "Waiting for messages\u{2026}".to_string(),
            "receive_message" => "Receiving messages\u{2026}".to_string(),
            other => format!("Using {other}\u{2026}"),
        }
    }

    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String {
        let str_field = |field: &str| -> String {
            input
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        match name {
            "send_message" => {
                let target = str_field("target");
                let content = str_field("content");
                let preview: String = content.chars().take(80).collect();
                if target.is_empty() {
                    preview
                } else {
                    format!("{target}: {preview}")
                }
            }
            _ => String::new(),
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        let binary_exists = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("chorus-stub-agent")))
            .map(|p| p.exists())
            .unwrap_or(false);

        Ok(RuntimeStatus {
            runtime: self.id().to_string(),
            installed: binary_exists,
            auth_status: Some(RuntimeAuthStatus::Authed),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec!["echo".to_string()])
    }
}
