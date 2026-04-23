use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};

pub mod backend;
pub mod discovery;
pub mod error;
pub mod format;
pub mod pairing;
pub mod serve;
mod types;

use backend::{Backend, ChorusBackend};
use types::*;

/// Default TCP port for the shared MCP bridge. Canonical across CLI defaults
/// (`chorus start --bridge-port`, `chorus serve --bridge-port`,
/// `chorus bridge-serve --listen`) so changing it touches one place.
pub const DEFAULT_BRIDGE_PORT: u16 = 4321;

/// Build the token-pair URL a runtime uses to reach the shared bridge:
/// `{endpoint}/token/{token}/mcp`. Trimming the trailing slash on `endpoint`
/// is a defensive chore the five drivers used to duplicate; put it here once.
pub fn token_mcp_url(endpoint: &str, token: &str) -> String {
    format!("{}/token/{}/mcp", endpoint.trim_end_matches('/'), token)
}

// ---------------------------------------------------------------------------
// ChatBridge
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ChatBridge {
    agent_id: String,
    backend: ChorusBackend,
    tool_router: ToolRouter<Self>,
}

impl ChatBridge {
    pub fn new(agent_id: String, server_url: String) -> Self {
        Self {
            agent_id,
            backend: ChorusBackend::new(server_url),
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
        Parameters(params): Parameters<SendMessageParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .send_message(
                &self.agent_id,
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
        _params: Parameters<EmptyParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .check_messages(&self.agent_id)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Read message history for a channel or DM. Supports pagination with before/after seq numbers."
    )]
    async fn read_history(
        &self,
        Parameters(params): Parameters<ReadHistoryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .read_history(
                &self.agent_id,
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
    async fn list_server(&self) -> Result<String, rmcp::ErrorData> {
        self.backend
            .list_channels(&self.agent_id)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "List tasks on a channel's task board. Returns tasks with their number (#t1, #t2...), title, status, and assignee."
    )]
    async fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .list_tasks(&self.agent_id, &params.channel, params.status.clone())
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Create one or more tasks on a channel's task board. Returns the created task numbers."
    )]
    async fn create_tasks(
        &self,
        Parameters(params): Parameters<CreateTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let task_titles: Vec<String> = params.tasks.iter().map(|t| t.title.clone()).collect();
        self.backend
            .create_tasks(&self.agent_id, &params.channel, task_titles)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Propose a task to the user. Use when the user delegates \
                       substantial work to you in chat (\"investigate the bug\", \
                       \"ship a feature\", \"write X\"). Do NOT use for quick \
                       questions. After you call this, WAIT — the user must \
                       click Create before you start work. You'll be woken in \
                       the new task sub-channel once accepted."
    )]
    async fn propose_task(
        &self,
        Parameters(params): Parameters<ProposeTaskParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .propose_task(&self.agent_id, &params.channel, &params.title)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Claim one or more tasks by their number. Returns which claims succeeded and which failed."
    )]
    async fn claim_tasks(
        &self,
        Parameters(params): Parameters<ClaimTasksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .claim_tasks(&self.agent_id, &params.channel, params.task_numbers.clone())
            .await
            .map_err(Into::into)
    }

    #[tool(description = "Release your claim on a task, setting it back to open.")]
    async fn unclaim_task(
        &self,
        Parameters(params): Parameters<UnclaimTaskParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .unclaim_task(&self.agent_id, &params.channel, params.task_number)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Update a task's progress status. Valid statuses: todo, in_progress, in_review, done."
    )]
    async fn update_task_status(
        &self,
        Parameters(params): Parameters<UpdateTaskStatusParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .update_task_status(
                &self.agent_id,
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
        Parameters(params): Parameters<UploadFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .upload_file(&self.agent_id, &params.file_path, &params.channel)
            .await
            .map_err(Into::into)
    }

    #[tool(
        description = "Download an attached image by its attachment ID and save it locally so you can view it. Returns the local file path."
    )]
    async fn view_file(
        &self,
        Parameters(params): Parameters<ViewFileParams>,
    ) -> Result<String, rmcp::ErrorData> {
        self.backend
            .view_file(&self.agent_id, &params.attachment_id)
            .await
            .map_err(Into::into)
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_bridge_constructs() {
        let bridge = ChatBridge::new("agent-1".into(), "http://localhost:3001".into());
        assert_eq!(bridge.agent_id, "agent-1");
    }
}
