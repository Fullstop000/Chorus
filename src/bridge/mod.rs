use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde_json::Value;
use std::time::Duration;

pub mod backend;
mod format;
pub mod session;
mod types;

use format::{format_attachments, format_target, to_local_time};
use types::*;

// ---------------------------------------------------------------------------
// ChatBridge
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ChatBridge {
    agent_id: String,
    server_url: String,
    client: reqwest::Client,
    tool_router: ToolRouter<Self>,
}

impl ChatBridge {
    pub fn new(agent_id: String, server_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");
        Self {
            agent_id,
            server_url,
            client,
            tool_router: Self::tool_router(),
        }
    }

    fn base_url(&self) -> String {
        format!(
            "{}/internal/agent/{}",
            self.server_url.trim_end_matches('/'),
            self.agent_id
        )
    }

    /// Poll the Chorus server for messages and return them in the MCP-facing text format.
    async fn receive_and_format(
        &self,
        block: bool,
        timeout_ms: u64,
    ) -> Result<String, rmcp::ErrorData> {
        let url = format!(
            "{}/receive?block={}&timeout={}",
            self.base_url(),
            block,
            timeout_ms
        );
        let res =
            self.client.get(&url).send().await.map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None)
            })?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        let messages = match data.get("messages").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => return Ok("No new messages.".into()),
        };

        let formatted: Vec<String> = messages
            .iter()
            .map(|m| {
                let target = format_target(m);
                let msg_id = m
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .map(|s| if s.len() >= 8 { &s[..8] } else { s })
                    .unwrap_or("-");
                let time = m
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(to_local_time)
                    .unwrap_or_else(|| "-".into());
                let sender_type = match m.get("sender_type").and_then(|v| v.as_str()) {
                    Some("agent") => " type=agent",
                    _ => "",
                };
                let sender = m
                    .get("sender_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let attach_suffix = format_attachments(m.get("attachments"));
                format!(
                    "[target={} msg={} time={}{}] @{}: {}{}",
                    target, msg_id, time, sender_type, sender, content, attach_suffix
                )
            })
            .collect();

        Ok(format!(
            "{}\n\nReply instructions:\n- For any human-visible reply, call send_message(target=\"<exact target from the header above>\", content=\"...\").\n- Reuse the exact target value from the header when you reply.\n- Do not output the reply as plain assistant text.",
            formatted.join("\n")
        ))
    }
}

#[tool_router]
impl ChatBridge {
    #[tool(
        description = "Send a message to a channel, DM, or thread. Use the target value from received messages to reply. Format: '#channel' for channels, 'dm:@peer' for DMs, '#channel:shortid' for threads in channels, 'dm:@peer:shortid' for threads in DMs."
    )]
    async fn send_message(
        &self,
        Parameters(params): Parameters<SendMessageParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let mut body = serde_json::json!({ "target": params.target, "content": params.content });
        if let Some(ids) = &params.attachment_ids {
            body["attachmentIds"] = serde_json::json!(ids);
        }
        let res = self
            .client
            .post(format!("{}/send", self.base_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let message_id = data.get("messageId").and_then(|v| v.as_str()).unwrap_or("");
        let short_id = if message_id.len() >= 8 {
            &message_id[..8]
        } else {
            message_id
        };
        let target = &params.target;
        let reply_hint = if !short_id.is_empty() {
            if target.contains(':') {
                format!(
                    " (to reply in this message's thread, use target \"{}\")",
                    target
                )
            } else {
                format!(
                    " (to reply in this message's thread, use target \"{}:{}\")",
                    target, short_id
                )
            }
        } else {
            String::new()
        };
        // Append sent content so trace viewers (Telescope) can display what was sent.
        // The model already knows its own content; this is confirmation, not redundancy.
        let content_suffix = if !params.content.is_empty() {
            format!("\nSent: {}", params.content)
        } else {
            String::new()
        };
        Ok(format!(
            "Message sent to {}. Message ID: {}{}{}",
            target, message_id, reply_hint, content_suffix
        ))
    }

    #[tool(
        description = "Check for new messages without waiting. Returns immediately with any pending messages, or 'No new messages.' if none are queued."
    )]
    async fn check_messages(
        &self,
        _params: Parameters<EmptyParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.receive_and_format(false, 0).await
    }

    #[tool(
        description = "Read message history for a channel, DM, or thread. Supports pagination with before/after seq numbers."
    )]
    async fn read_history(
        &self,
        Parameters(params): Parameters<ReadHistoryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let limit = params.limit.unwrap_or(50).min(100);
        let mut url = format!(
            "{}/history?channel={}&limit={}",
            self.base_url(),
            urlencoding::encode(&params.channel),
            limit
        );
        if let Some(b) = params.before {
            url.push_str(&format!("&before={}", b));
        }
        if let Some(a) = params.after {
            url.push_str(&format!("&after={}", a));
        }

        let res =
            self.client.get(&url).send().await.map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None)
            })?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let messages = match data.get("messages").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => return Ok("No messages in this channel.".into()),
        };

        let formatted: Vec<String> = messages
            .iter()
            .map(|m| {
                let sender_type = match m.get("senderType").and_then(|v| v.as_str()) {
                    Some("agent") => " type=agent",
                    _ => "",
                };
                let time = m
                    .get("createdAt")
                    .and_then(|v| v.as_str())
                    .map(to_local_time)
                    .unwrap_or_else(|| "-".into());
                let msg_id = m
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| if s.len() >= 8 { &s[..8] } else { s })
                    .unwrap_or("-");
                let seq = m
                    .get("seq")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into());
                let sender = m
                    .get("senderName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let attach_suffix = format_attachments(m.get("attachments"));
                format!(
                    "[seq={} msg={} time={}{}] @{}: {}{}",
                    seq, msg_id, time, sender_type, sender, content, attach_suffix
                )
            })
            .collect();

        let mut footer = String::new();
        if data
            .get("historyLimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let msg = data
                .get("historyLimitMessage")
                .and_then(|v| v.as_str())
                .unwrap_or("Message history is limited on this plan.");
            footer = format!("\n\n--- {} ---", msg);
        } else if data
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !messages.is_empty()
        {
            if params.after.is_some() {
                let max_seq = messages
                    .last()
                    .and_then(|m| m.get("seq").and_then(|v| v.as_i64()))
                    .unwrap_or(0);
                footer = format!(
                    "\n\n--- {} messages shown. Use after={} to load more recent messages. ---",
                    messages.len(),
                    max_seq
                );
            } else {
                let min_seq = messages
                    .first()
                    .and_then(|m| m.get("seq").and_then(|v| v.as_i64()))
                    .unwrap_or(0);
                footer = format!(
                    "\n\n--- {} messages shown. Use before={} to load older messages. ---",
                    messages.len(),
                    min_seq
                );
            }
        }

        let channel = &params.channel;
        let mut header = format!(
            "## Message History for {} ({} messages)",
            channel,
            messages.len()
        );
        let last_read_seq = data
            .get("last_read_seq")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if last_read_seq > 0 && params.after.is_none() && params.before.is_none() {
            header.push_str(&format!(
                "\nYour last read position: seq {}. Use read_history(channel=\"{}\", after={}) to see only unread messages.",
                last_read_seq, channel, last_read_seq
            ));
        }

        Ok(format!("{}\n\n{}{}", header, formatted.join("\n"), footer))
    }

    #[tool(
        description = "List all channels in this server, including which ones you have joined, plus all agents and humans. Use this to discover who and where you can message."
    )]
    async fn list_server(&self) -> Result<String, rmcp::ErrorData> {
        let res = self
            .client
            .get(format!("{}/server", self.base_url()))
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        let mut text = "## Server\n\n".to_string();

        text.push_str("### Channels\n");
        text.push_str("Use `#channel-name` with send_message to post in a channel. `joined` means you currently belong to that channel.\n");

        match data.get("channels").and_then(|v| v.as_array()) {
            Some(channels) if !channels.is_empty() => {
                for ch in channels {
                    let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let joined = ch.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
                    let status = if joined { "joined" } else { "not joined" };
                    if let Some(desc) = ch.get("description").and_then(|v| v.as_str()) {
                        if !desc.is_empty() {
                            text.push_str(&format!(
                                "  - #{} [{}] \u{2014} {}\n",
                                name, status, desc
                            ));
                            continue;
                        }
                    }
                    text.push_str(&format!("  - #{} [{}]\n", name, status));
                }
            }
            _ => {
                text.push_str("  (none)\n");
            }
        }

        text.push_str("\n### Agents\n");
        text.push_str("Other AI agents in this server.\n");
        match data.get("agents").and_then(|v| v.as_array()) {
            Some(agents) if !agents.is_empty() => {
                for a in agents {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = a
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    text.push_str(&format!("  - @{} ({})\n", name, status));
                }
            }
            _ => {
                text.push_str("  (none)\n");
            }
        }

        text.push_str("\n### Humans\n");
        text.push_str("To start a new DM: send_message(target=\"dm:@name\"). To reply in an existing DM: reuse the target from received messages.\n");
        match data.get("humans").and_then(|v| v.as_array()) {
            Some(humans) if !humans.is_empty() => {
                for u in humans {
                    let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    text.push_str(&format!("  - @{}\n", name));
                }
            }
            _ => {
                text.push_str("  (none)\n");
            }
        }

        Ok(text)
    }

    #[tool(
        description = "List tasks on a channel's task board. Returns tasks with their number (#t1, #t2...), title, status, and assignee."
    )]
    async fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let status = params.status.as_deref().unwrap_or("all");
        let mut url = format!(
            "{}/tasks?channel={}",
            self.base_url(),
            urlencoding::encode(&params.channel)
        );
        if status != "all" {
            url.push_str(&format!("&status={}", urlencoding::encode(status)));
        }

        let res =
            self.client.get(&url).send().await.map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None)
            })?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let tasks = match data.get("tasks").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => {
                let status_filter = if status != "all" {
                    format!(" {}", status)
                } else {
                    String::new()
                };
                return Ok(format!("No{} tasks in {}.", status_filter, params.channel));
            }
        };

        let formatted: Vec<String> = tasks
            .iter()
            .map(|t| {
                let task_num = t.get("taskNumber").and_then(|v| v.as_i64()).unwrap_or(0);
                let st = t.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                let assignee = t
                    .get("claimedByName")
                    .and_then(|v| v.as_str())
                    .map(|n| format!(" \u{2192} @{}", n))
                    .unwrap_or_default();
                let creator = t
                    .get("createdByName")
                    .and_then(|v| v.as_str())
                    .map(|n| format!(" (by @{})", n))
                    .unwrap_or_default();
                format!(
                    "#t{} [{}] \"{}\"{}{}",
                    task_num, st, title, assignee, creator
                )
            })
            .collect();

        Ok(format!(
            "## Task Board for {} ({} tasks)\n\n{}",
            params.channel,
            tasks.len(),
            formatted.join("\n")
        ))
    }

    #[tool(
        description = "Create one or more tasks on a channel's task board. Returns the created task numbers."
    )]
    async fn create_tasks(
        &self,
        Parameters(params): Parameters<CreateTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let tasks_json: Vec<Value> = params
            .tasks
            .iter()
            .map(|t| serde_json::json!({ "title": t.title }))
            .collect();
        let body = serde_json::json!({ "channel": params.channel, "tasks": tasks_json });
        let res = self
            .client
            .post(format!("{}/tasks", self.base_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let created_tasks = data
            .get("tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let lines: Vec<String> = created_tasks
            .iter()
            .map(|t| {
                let num = t.get("taskNumber").and_then(|v| v.as_i64()).unwrap_or(0);
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                format!("#t{} \"{}\"", num, title)
            })
            .collect();

        Ok(format!(
            "Created {} task(s) in {}:\n{}",
            created_tasks.len(),
            params.channel,
            lines.join("\n")
        ))
    }

    #[tool(
        description = "Claim one or more tasks by their number. Returns which claims succeeded and which failed."
    )]
    async fn claim_tasks(
        &self,
        Parameters(params): Parameters<ClaimTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let body =
            serde_json::json!({ "channel": params.channel, "task_numbers": params.task_numbers });
        let res = self
            .client
            .post(format!("{}/tasks/claim", self.base_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let results = data
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let lines: Vec<String> = results
            .iter()
            .map(|r| {
                let num = r.get("taskNumber").and_then(|v| v.as_i64()).unwrap_or(0);
                let success = r.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                if success {
                    format!("#t{}: claimed", num)
                } else {
                    let reason = r
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("already claimed");
                    format!("#t{}: FAILED \u{2014} {}", num, reason)
                }
            })
            .collect();

        let succeeded = results
            .iter()
            .filter(|r| r.get("success").and_then(|v| v.as_bool()).unwrap_or(false))
            .count();
        let failed = results.len() - succeeded;
        let mut summary = format!("{} claimed", succeeded);
        if failed > 0 {
            summary.push_str(&format!(", {} failed", failed));
        }

        Ok(format!(
            "Claim results ({}):\n{}",
            summary,
            lines.join("\n")
        ))
    }

    #[tool(description = "Release your claim on a task, setting it back to open.")]
    async fn unclaim_task(
        &self,
        Parameters(params): Parameters<UnclaimTaskParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let body =
            serde_json::json!({ "channel": params.channel, "task_number": params.task_number });
        let res = self
            .client
            .post(format!("{}/tasks/unclaim", self.base_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        Ok(format!(
            "#t{} unclaimed \u{2014} now open.",
            params.task_number
        ))
    }

    #[tool(
        description = "Update a task's progress status. Valid statuses: todo, in_progress, in_review, done."
    )]
    async fn update_task_status(
        &self,
        Parameters(params): Parameters<UpdateTaskStatusParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let body = serde_json::json!({
            "channel": params.channel,
            "task_number": params.task_number,
            "status": params.status
        });
        let res = self
            .client
            .post(format!("{}/tasks/update-status", self.base_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        Ok(format!(
            "#t{} moved to {}.",
            params.task_number, params.status
        ))
    }

    #[tool(
        description = "Upload an image file to attach to a message. Returns an attachment ID for use with send_message. Supported: JPEG, PNG, GIF, WebP. Max 5MB."
    )]
    async fn upload_file(
        &self,
        Parameters(params): Parameters<UploadFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let path = std::path::Path::new(&params.file_path);
        if !path.exists() {
            return Ok(format!("Error: File not found: {}", params.file_path));
        }
        let metadata = std::fs::metadata(&params.file_path).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Cannot read file: {}", e), None)
        })?;
        if metadata.len() > 5 * 1024 * 1024 {
            return Ok(format!(
                "Error: File too large ({:.1}MB). Max 5MB.",
                metadata.len() as f64 / 1024.0 / 1024.0
            ));
        }

        // Resolve channel
        let resolve_res = self
            .client
            .post(format!("{}/resolve-channel", self.base_url()))
            .json(&serde_json::json!({ "target": params.channel }))
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !resolve_res.status().is_success() {
            let status = resolve_res.status();
            let body = resolve_res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let resolve_data: Value = resolve_res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        let channel_id = resolve_data
            .get("channelId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error("No channelId in response".to_string(), None)
            })?
            .to_string();

        // Read file
        let file_bytes = std::fs::read(&params.file_path).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Cannot read file: {}", e), None)
        })?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let mime_type = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => "application/octet-stream",
        };

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename.clone())
            .mime_str(mime_type)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("MIME error: {}", e), None))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("channelId", channel_id);

        let res = self
            .client
            .post(format!("{}/upload", self.base_url()))
            .multipart(form)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Upload failed: {}", e), None))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))?;

        if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            return Ok(format!("Error: {}", err));
        }

        let uploaded_filename = data
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or(&filename);
        let size_bytes = data
            .get("sizeBytes")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let id = data.get("id").and_then(|v| v.as_str()).unwrap_or("?");

        Ok(format!(
            "File uploaded: {} ({:.1}KB)\nAttachment ID: {}\n\nUse this ID in send_message's attachment_ids parameter to include it in a message.",
            uploaded_filename,
            size_bytes / 1024.0,
            id
        ))
    }

    #[tool(
        description = "Download an attached image by its attachment ID and save it locally so you can view it. Returns the local file path."
    )]
    async fn view_file(
        &self,
        Parameters(params): Parameters<ViewFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        // Validate attachment_id to prevent path traversal. Only allow characters
        // that appear in UUID strings (hex digits and dashes) plus underscores.
        // This must happen before any filesystem operation.
        if params.attachment_id.is_empty()
            || !params
                .attachment_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(rmcp::ErrorData::invalid_params(
                "Invalid attachment_id: must contain only alphanumeric characters, dashes, and underscores",
                None,
            ));
        }

        // Cache attachments inside the agent workspace so sandboxed agents can read them.
        let cache_dir = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".chorus")
            .join("attachments");
        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Cannot create cache dir: {}", e), None)
        })?;

        // Check for cached file
        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&params.attachment_id) {
                    let cached_path = entry.path().to_string_lossy().to_string();
                    return Ok(format!(
                        "File already cached at: {}\n\nUse your Read tool to view this image.",
                        cached_path
                    ));
                }
            }
        }

        let url = format!(
            "{}/api/attachments/{}",
            self.server_url.trim_end_matches('/'),
            params.attachment_id
        );
        let res = self.client.get(&url).send().await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Download failed: {}", e), None)
        })?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", status, body),
                None,
            ));
        }

        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let ext = match content_type.as_str() {
            "image/jpeg" => ".jpg",
            "image/png" => ".png",
            "image/gif" => ".gif",
            "image/webp" => ".webp",
            _ => ".bin",
        };

        let file_path = cache_dir.join(format!("{}{}", params.attachment_id, ext));
        let bytes = res.bytes().await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Download failed: {}", e), None)
        })?;

        std::fs::write(&file_path, &bytes)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Write failed: {}", e), None))?;

        Ok(format!(
            "Downloaded to: {}\n\nUse your Read tool to view this image.",
            file_path.to_string_lossy()
        ))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler impl
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for ChatBridge {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Chat bridge for agent communication".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run_bridge(agent_id: String, server_url: String) -> anyhow::Result<()> {
    let bridge = ChatBridge::new(agent_id, server_url);
    let service = bridge.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// Returns true if the given attachment_id passes the view_file validation rules.
    fn is_valid_attachment_id(id: &str) -> bool {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    #[test]
    fn valid_uuid_passes() {
        // Standard UUID format used by Chorus attachments
        assert!(is_valid_attachment_id(
            "550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[test]
    fn alphanumeric_with_dashes_and_underscores_passes() {
        assert!(is_valid_attachment_id("abc123"));
        assert!(is_valid_attachment_id("abc-123"));
        assert!(is_valid_attachment_id("abc_123"));
        assert!(is_valid_attachment_id("ABC-DEF_001"));
    }

    #[test]
    fn path_traversal_dotdot_slash_rejected() {
        assert!(!is_valid_attachment_id("../etc/passwd"));
        assert!(!is_valid_attachment_id("..%2Fetc%2Fpasswd"));
    }

    #[test]
    fn forward_slash_rejected() {
        assert!(!is_valid_attachment_id("foo/bar"));
    }

    #[test]
    fn backslash_rejected() {
        assert!(!is_valid_attachment_id("foo\\bar"));
    }

    #[test]
    fn dot_alone_rejected() {
        assert!(!is_valid_attachment_id(".hidden"));
        assert!(!is_valid_attachment_id("some.file"));
    }

    #[test]
    fn empty_string_rejected() {
        assert!(!is_valid_attachment_id(""));
    }

    #[test]
    fn whitespace_rejected() {
        assert!(!is_valid_attachment_id("foo bar"));
        assert!(!is_valid_attachment_id("foo\tbar"));
        assert!(!is_valid_attachment_id("foo\nbar"));
    }

    #[test]
    fn null_byte_rejected() {
        assert!(!is_valid_attachment_id("foo\0bar"));
    }
}
