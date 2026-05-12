use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};

pub mod backend;
pub mod client;
pub mod discovery;
pub mod error;
pub mod format;
pub mod protocol;
pub mod serve;
mod types;

use backend::{Backend, ChorusBackend};
use types::*;

/// Default TCP port for the shared MCP bridge. Canonical across CLI defaults
/// (`chorus start --bridge-port`, `chorus serve --bridge-port`,
/// `chorus bridge-serve --listen`) so changing it touches one place.
pub const DEFAULT_BRIDGE_PORT: u16 = 4321;

/// Reject agent_keys that could pivot to other endpoints or traverse the
/// filesystem, but accept the full set of names Chorus allows elsewhere.
///
/// Chorus only enforces `!name.is_empty()` at create time (see
/// `handle_create_agent` in `src/server/handlers/agents.rs`), so existing
/// agents may have spaces, dots, or Unicode in their names.
pub fn agent_key_is_safe(key: &str) -> bool {
    !(key.trim().is_empty()
        || key.len() > 256
        || key.contains('/')
        || key.contains('\\')
        || key.contains("..")
        || key.chars().any(|c| c.is_control()))
}

/// Extract the agent identity from the `X-Agent-Id` HTTP header injected
/// by rmcp's `StreamableHttpService` into request extensions.
///
/// Every tool handler calls this to obtain the per-request agent_id rather
/// than relying on a struct field.
fn extract_agent_id(parts: &axum::http::request::Parts) -> Result<String, rmcp::ErrorData> {
    let header = parts
        .headers
        .get("X-Agent-Id")
        .ok_or_else(|| {
            rmcp::ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Missing X-Agent-Id header".to_string(),
                None,
            )
        })?
        .to_str()
        .map_err(|_| {
            rmcp::ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Invalid X-Agent-Id header encoding".to_string(),
                None,
            )
        })?;

    if !agent_key_is_safe(header) {
        return Err(rmcp::ErrorData::new(
            rmcp::model::ErrorCode::INVALID_PARAMS,
            "Invalid X-Agent-Id value".to_string(),
            None,
        ));
    }

    Ok(header.to_string())
}

// ---------------------------------------------------------------------------
// ChatBridge
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ChatBridge {
    backend: ChorusBackend,
    tool_router: ToolRouter<Self>,
}

impl ChatBridge {
    pub fn new(server_url: String) -> Self {
        Self::with_token(server_url, None)
    }

    pub fn with_token(server_url: String, bearer_token: Option<String>) -> Self {
        Self {
            backend: ChorusBackend::with_token(server_url, bearer_token),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ChatBridge {
    #[tool(
        description = "Send a message to a channel or DM. Use the target value from received messages to reply. Format: '#channel' for channels, 'dm:@peer' for DMs."
    )]
    async fn send_message(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<SendMessageParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .send_message(
                &agent_id,
                &params.target,
                &params.content,
                params.attachment_ids.clone(),
            )
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Check for new messages without waiting. Returns immediately with any pending messages, or 'No new messages.' if none are queued."
    )]
    async fn check_messages(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        _params: Parameters<EmptyParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .check_messages(&agent_id)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Read message history for a channel or DM. Supports pagination with before/after seq numbers."
    )]
    async fn read_history(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<ReadHistoryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .read_history(
                &agent_id,
                &params.channel,
                params.limit,
                params.before,
                params.after,
            )
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "List all channels in this server, including which ones you have joined, plus all agents and humans. Use this to discover who and where you can message."
    )]
    async fn list_server(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .list_channels(&agent_id)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "List tasks on a channel's task board. Returns tasks with their number (#t1, #t2...), title, status, and assignee."
    )]
    async fn list_tasks(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .list_tasks(&agent_id, &params.channel, params.status.clone())
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Create one or more tasks on a channel's task board. Returns the created task numbers."
    )]
    async fn create_tasks(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<CreateTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        let task_titles: Vec<String> = params.tasks.iter().map(|t| t.title.clone()).collect();
        self.backend
            .create_tasks(&agent_id, &params.channel, task_titles)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Claim one or more tasks by their number. Returns which claims succeeded and which failed."
    )]
    async fn claim_tasks(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<ClaimTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .claim_tasks(&agent_id, &params.channel, params.task_numbers.clone())
            .await
            .map_err(Into::into)
    }

    #[tool(description = "Release your claim on a task, setting it back to open.")]
    async fn unclaim_task(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<UnclaimTaskParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .unclaim_task(&agent_id, &params.channel, params.task_number)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Update a task's progress status. Valid statuses: todo, in_progress, in_review, done."
    )]
    async fn update_task_status(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<UpdateTaskStatusParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .update_task_status(
                &agent_id,
                &params.channel,
                params.task_number,
                &params.status,
            )
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Upload an image file to attach to a message. Returns an attachment ID for use with send_message. Supported: JPEG, PNG, GIF, WebP. Max 5MB."
    )]
    async fn upload_file(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<UploadFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .upload_file(&agent_id, &params.file_path, &params.channel)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Download an attached image by its attachment ID and save it locally so you can view it. Returns the local file path."
    )]
    async fn view_file(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<ViewFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;
        self.backend
            .view_file(&agent_id, &params.attachment_id)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Submit a decision for the human to pick. REQUIRED for any incoming request that asks you to render a verdict, judge, or pick between concrete alternatives — PR review outcomes (merge/approve/request-changes), A-vs-B implementation choices, config flags, \"should I X or Y\" questions. Do NOT post your verdict via send_message; emit this tool with options + a recommended_key, then end your turn. The human's pick arrives as your next session prompt with the picked option's full body."
    )]
    async fn dispatch_decision(
        &self,
        Extension(parts): Extension<axum::http::request::Parts>,
        Parameters(params): Parameters<CreateDecisionParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_id = extract_agent_id(&parts)?;

        // Light structural validation at the bridge boundary so a malformed
        // payload doesn't 500 the handler. The trace target is the agent's
        // PROACTIVE EMISSION; we surface validator errors back as
        // rmcp::ErrorData so the agent learns and retries (per CLAUDE.md
        // "fail loudly with context").
        validate_decision_payload(&params)?;

        let payload = serde_json::json!({
            "headline": params.headline,
            "question": params.question,
            "options": params.options.iter().map(|o| serde_json::json!({
                "key": o.key,
                "label": o.label,
                "body": o.body,
            })).collect::<Vec<_>>(),
            "recommended_key": params.recommended_key,
            "context": params.context,
        });

        self.backend
            .create_decision(&agent_id, payload)
            .await
            .map_err(Into::into)
    }
}

/// Minimal structural validator. Bridge boundary defense against payloads
/// that would 500 the stub handler. Intentionally permissive on content —
/// the goal is to capture the agent's emission, not gatekeep quality.
fn validate_decision_payload(p: &CreateDecisionParams) -> Result<(), rmcp::ErrorData> {
    fn invalid(msg: impl Into<String>) -> rmcp::ErrorData {
        rmcp::ErrorData::new(rmcp::model::ErrorCode::INVALID_PARAMS, msg.into(), None)
    }
    if p.headline.trim().is_empty() {
        return Err(invalid("headline is empty"));
    }
    if p.headline.chars().count() > 80 {
        return Err(invalid("headline exceeds 80 chars"));
    }
    if p.question.trim().is_empty() {
        return Err(invalid("question is empty"));
    }
    if p.question.chars().count() > 120 {
        return Err(invalid("question exceeds 120 chars"));
    }
    if p.options.len() < 2 || p.options.len() > 6 {
        return Err(invalid("options must have 2..=6 entries"));
    }
    let mut keys = std::collections::HashSet::new();
    for o in &p.options {
        if o.key.trim().is_empty() {
            return Err(invalid("option key is empty"));
        }
        if o.key.chars().count() > 2 {
            return Err(invalid(format!("option key '{}' exceeds 2 chars", o.key)));
        }
        if !keys.insert(o.key.clone()) {
            return Err(invalid(format!("duplicate option key '{}'", o.key)));
        }
        if o.label.trim().is_empty() {
            return Err(invalid(format!("option '{}' label is empty", o.key)));
        }
        if o.label.chars().count() > 40 {
            return Err(invalid(format!(
                "option '{}' label exceeds 40 chars",
                o.key
            )));
        }
        if o.body.chars().count() > 2048 {
            return Err(invalid(format!(
                "option '{}' body exceeds 2048 chars",
                o.key
            )));
        }
    }
    if !keys.contains(&p.recommended_key) {
        return Err(invalid(format!(
            "recommended_key '{}' is not one of the option keys",
            p.recommended_key
        )));
    }
    if let Some(ctx) = &p.context {
        if ctx.chars().count() > 4096 {
            return Err(invalid("context exceeds 4096 chars"));
        }
    }
    Ok(())
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_bridge_constructs() {
        let _bridge = ChatBridge::new("http://localhost:3001".into());
    }

    #[test]
    fn agent_key_accepts_chorus_names() {
        assert!(agent_key_is_safe("bot1"));
        assert!(agent_key_is_safe("Agent Smith"));
        assert!(agent_key_is_safe("bot.with.dots"));
        assert!(agent_key_is_safe("unicode-名字"));
        assert!(agent_key_is_safe("a"));
        assert!(agent_key_is_safe(&"x".repeat(256)));
    }

    #[test]
    fn agent_key_rejects_dangerous_input() {
        assert!(!agent_key_is_safe(""));
        assert!(!agent_key_is_safe(&"x".repeat(257)));
        assert!(!agent_key_is_safe("../etc/passwd"));
        assert!(!agent_key_is_safe("a/b"));
        assert!(!agent_key_is_safe("a\\b"));
        assert!(!agent_key_is_safe("with\0null"));
        assert!(!agent_key_is_safe("with\nnewline"));
        assert!(!agent_key_is_safe(".."));
    }
}
