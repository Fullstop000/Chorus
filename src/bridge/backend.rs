use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

use super::format::{format_attachments, format_target, to_local_time};

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// Abstracts the IM backend so the bridge can be used with Chorus, Slack,
/// Discord, or any other platform without changing the tool layer.
///
/// `agent_key` is the first parameter on every method because the backend
/// is shared across agents — the caller specifies which agent is acting.
///
/// ## Known simplifications (Phase 1)
///
/// - **Error type:** Methods return `rmcp::ErrorData` directly. A future iteration
///   should return `BridgeError` (from `crate::bridge::error`) and convert to
///   `rmcp::ErrorData` at the MCP handler boundary.
/// - **Return type:** Most methods return pre-formatted `String`. A future iteration
///   will introduce typed response structs to decouple backends from MCP presentation.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Send a message to a channel or DM target.
    async fn send_message(
        &self,
        agent_key: &str,
        target: &str,
        content: &str,
        attachment_ids: Option<Vec<String>>,
    ) -> Result<String, rmcp::ErrorData>;

    /// Receive new messages for this agent (blocking or non-blocking).
    async fn receive_messages(
        &self,
        agent_key: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<String, rmcp::ErrorData>;

    /// Check for new messages (non-blocking convenience).
    async fn check_messages(&self, agent_key: &str) -> Result<String, rmcp::ErrorData>;

    /// Read message history from a channel.
    async fn read_history(
        &self,
        agent_key: &str,
        channel: &str,
        limit: Option<u32>,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<String, rmcp::ErrorData>;

    /// List available channels.
    async fn list_channels(&self, agent_key: &str) -> Result<String, rmcp::ErrorData>;

    /// Get server info.
    async fn server_info(&self, agent_key: &str) -> Result<Value, rmcp::ErrorData>;

    /// List tasks in a channel.
    async fn list_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        status: Option<String>,
    ) -> Result<String, rmcp::ErrorData>;

    /// Create tasks in a channel.
    async fn create_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        tasks: Vec<String>,
    ) -> Result<String, rmcp::ErrorData>;

    /// Claim tasks.
    async fn claim_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        task_numbers: Vec<i64>,
    ) -> Result<String, rmcp::ErrorData>;

    /// Unclaim a task.
    async fn unclaim_task(
        &self,
        agent_key: &str,
        channel: &str,
        task_number: i64,
    ) -> Result<String, rmcp::ErrorData>;

    /// Update task status.
    async fn update_task_status(
        &self,
        agent_key: &str,
        channel: &str,
        task_number: i64,
        status: &str,
    ) -> Result<String, rmcp::ErrorData>;

    /// Upload a file.
    async fn upload_file(
        &self,
        agent_key: &str,
        file_path: &str,
        channel: &str,
    ) -> Result<String, rmcp::ErrorData>;

    /// View/download a file attachment.
    async fn view_file(
        &self,
        agent_key: &str,
        attachment_id: &str,
    ) -> Result<String, rmcp::ErrorData>;
}

// ---------------------------------------------------------------------------
// ChorusBackend
// ---------------------------------------------------------------------------

/// Chorus-specific implementation of [`Backend`].
///
/// Delegates each operation to the Chorus HTTP API using the same request
/// logic that `ChatBridge` uses today.
#[derive(Clone)]
pub struct ChorusBackend {
    server_url: String,
    client: reqwest::Client,
}

impl ChorusBackend {
    pub fn new(server_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        Self { server_url, client }
    }

    /// Build the per-agent base URL for internal agent endpoints.
    fn base_url(&self, agent_key: &str) -> String {
        format!(
            "{}/internal/agent/{}",
            self.server_url.trim_end_matches('/'),
            agent_key
        )
    }
}

#[async_trait]
impl Backend for ChorusBackend {
    async fn send_message(
        &self,
        agent_key: &str,
        target: &str,
        content: &str,
        attachment_ids: Option<Vec<String>>,
    ) -> Result<String, rmcp::ErrorData> {
        let mut body = serde_json::json!({ "target": target, "content": content });
        if let Some(ids) = &attachment_ids {
            body["attachmentIds"] = serde_json::json!(ids);
        }

        let res = self
            .client
            .post(format!("{}/send", self.base_url(agent_key)))
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
        let content_suffix = if !content.is_empty() {
            format!("\nSent: {}", content)
        } else {
            String::new()
        };
        Ok(format!(
            "Message sent to {}. Message ID: {}{}{}",
            target, message_id, reply_hint, content_suffix
        ))
    }

    async fn receive_messages(
        &self,
        agent_key: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<String, rmcp::ErrorData> {
        let url = format!(
            "{}/receive?block={}&timeout={}",
            self.base_url(agent_key),
            block,
            timeout_ms
        );
        let res = self.client.get(&url).send().await.map_err(|e| {
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

    async fn check_messages(&self, agent_key: &str) -> Result<String, rmcp::ErrorData> {
        self.receive_messages(agent_key, false, 0).await
    }

    async fn read_history(
        &self,
        agent_key: &str,
        channel: &str,
        limit: Option<u32>,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<String, rmcp::ErrorData> {
        let limit = limit.unwrap_or(50).min(100);
        let mut url = format!(
            "{}/history?channel={}&limit={}",
            self.base_url(agent_key),
            urlencoding::encode(channel),
            limit
        );
        if let Some(b) = before {
            url.push_str(&format!("&before={}", b));
        }
        if let Some(a) = after {
            url.push_str(&format!("&after={}", a));
        }

        let res = self.client.get(&url).send().await.map_err(|e| {
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
            if after.is_some() {
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

        let mut header = format!(
            "## Message History for {} ({} messages)",
            channel,
            messages.len()
        );
        let last_read_seq = data
            .get("last_read_seq")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if last_read_seq > 0 && after.is_none() && before.is_none() {
            header.push_str(&format!(
                "\nYour last read position: seq {}. Use read_history(channel=\"{}\", after={}) to see only unread messages.",
                last_read_seq, channel, last_read_seq
            ));
        }

        Ok(format!("{}\n\n{}{}", header, formatted.join("\n"), footer))
    }

    async fn list_channels(&self, agent_key: &str) -> Result<String, rmcp::ErrorData> {
        let data = self.server_info(agent_key).await?;

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

    async fn server_info(&self, agent_key: &str) -> Result<Value, rmcp::ErrorData> {
        let res = self
            .client
            .get(format!("{}/server", self.base_url(agent_key)))
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

        res.json()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid JSON: {}", e), None))
    }

    async fn list_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        status: Option<String>,
    ) -> Result<String, rmcp::ErrorData> {
        let status_str = status.as_deref().unwrap_or("all");
        let mut url = format!(
            "{}/tasks?channel={}",
            self.base_url(agent_key),
            urlencoding::encode(channel)
        );
        if status_str != "all" {
            url.push_str(&format!("&status={}", urlencoding::encode(status_str)));
        }

        let res = self.client.get(&url).send().await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None)
        })?;

        if !res.status().is_success() {
            let http_status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", http_status, body),
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
                let status_filter = if status_str != "all" {
                    format!(" {}", status_str)
                } else {
                    String::new()
                };
                return Ok(format!("No{} tasks in {}.", status_filter, channel));
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
            channel,
            tasks.len(),
            formatted.join("\n")
        ))
    }

    async fn create_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        tasks: Vec<String>,
    ) -> Result<String, rmcp::ErrorData> {
        let tasks_json: Vec<Value> = tasks
            .iter()
            .map(|t| serde_json::json!({ "title": t }))
            .collect();
        let body = serde_json::json!({ "channel": channel, "tasks": tasks_json });

        let res = self
            .client
            .post(format!("{}/tasks", self.base_url(agent_key)))
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
            channel,
            lines.join("\n")
        ))
    }

    async fn claim_tasks(
        &self,
        agent_key: &str,
        channel: &str,
        task_numbers: Vec<i64>,
    ) -> Result<String, rmcp::ErrorData> {
        let body = serde_json::json!({ "channel": channel, "task_numbers": task_numbers });

        let res = self
            .client
            .post(format!("{}/tasks/claim", self.base_url(agent_key)))
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

    async fn unclaim_task(
        &self,
        agent_key: &str,
        channel: &str,
        task_number: i64,
    ) -> Result<String, rmcp::ErrorData> {
        let body = serde_json::json!({ "channel": channel, "task_number": task_number });

        let res = self
            .client
            .post(format!("{}/tasks/unclaim", self.base_url(agent_key)))
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

        Ok(format!("#t{} unclaimed \u{2014} now open.", task_number))
    }

    async fn update_task_status(
        &self,
        agent_key: &str,
        channel: &str,
        task_number: i64,
        status: &str,
    ) -> Result<String, rmcp::ErrorData> {
        let body = serde_json::json!({
            "channel": channel,
            "task_number": task_number,
            "status": status
        });

        let res = self
            .client
            .post(format!(
                "{}/tasks/update-status",
                self.base_url(agent_key)
            ))
            .json(&body)
            .send()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Request failed: {}", e), None))?;

        if !res.status().is_success() {
            let http_status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(rmcp::ErrorData::internal_error(
                format!("Server returned {}: {}", http_status, body),
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

        Ok(format!("#t{} moved to {}.", task_number, status))
    }

    async fn upload_file(
        &self,
        agent_key: &str,
        file_path: &str,
        channel: &str,
    ) -> Result<String, rmcp::ErrorData> {
        let path = std::path::Path::new(file_path);
        if !path.exists() {
            return Ok(format!("Error: File not found: {}", file_path));
        }
        let metadata = std::fs::metadata(file_path).map_err(|e| {
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
            .post(format!("{}/resolve-channel", self.base_url(agent_key)))
            .json(&serde_json::json!({ "target": channel }))
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
        let file_bytes = std::fs::read(file_path).map_err(|e| {
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
            .post(format!("{}/upload", self.base_url(agent_key)))
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

    async fn view_file(
        &self,
        _agent_key: &str,
        attachment_id: &str,
    ) -> Result<String, rmcp::ErrorData> {
        // Validate attachment_id to prevent path traversal. Only allow characters
        // that appear in UUID strings (hex digits and dashes) plus underscores.
        if attachment_id.is_empty()
            || !attachment_id
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

        // Check for cached file. We require an exact `{id}{ext}` match rather
        // than a `starts_with(id)` scan so that e.g. attachment_id "abc" does
        // not accidentally collide with a cached "abc123.png".
        const KNOWN_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".webp", ".pdf", ".bin"];
        for ext in KNOWN_EXTENSIONS {
            let candidate = cache_dir.join(format!("{}{}", attachment_id, ext));
            if candidate.exists() {
                return Ok(format!(
                    "File already cached at: {}\n\nUse your Read tool to view this image.",
                    candidate.to_string_lossy()
                ));
            }
        }

        // view_file uses the public /api/attachments endpoint, not the
        // agent-scoped base_url. _agent_key is unused for the download itself
        // but kept in the trait signature for uniformity.
        let url = format!(
            "{}/api/attachments/{}",
            self.server_url.trim_end_matches('/'),
            attachment_id
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

        let file_path = cache_dir.join(format!("{}{}", attachment_id, ext));
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chorus_backend_constructs() {
        let backend = ChorusBackend::new("http://localhost:3001".to_string());
        assert_eq!(
            backend.base_url("agent-1"),
            "http://localhost:3001/internal/agent/agent-1"
        );
    }

    #[test]
    fn chorus_backend_trims_trailing_slash() {
        let backend = ChorusBackend::new("http://localhost:3001/".to_string());
        assert_eq!(
            backend.base_url("bot-x"),
            "http://localhost:3001/internal/agent/bot-x"
        );
    }

    #[test]
    fn backend_trait_is_object_safe() {
        // Verify Backend can be used as a trait object.
        fn _assert_object_safe(_b: &dyn Backend) {}
        let backend = ChorusBackend::new("http://localhost:3001".to_string());
        _assert_object_safe(&backend);
    }

    #[test]
    fn chorus_backend_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<ChorusBackend>();
    }
}
