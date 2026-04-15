use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

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
// ChorusBackend stub
// ---------------------------------------------------------------------------

/// Chorus-specific implementation of [`Backend`].
///
/// Delegates each operation to the Chorus HTTP API. The actual trait
/// implementation will be added when `ChatBridge` is refactored to use
/// this backend.
#[allow(unused)]
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
}
