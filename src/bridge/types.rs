use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SendMessageParams {
    /// Where to send. Format: '#channel' for channels, 'dm:@name' for DMs.
    pub(super) target: String,
    /// The message content
    pub(super) content: String,
    /// Optional attachment IDs from upload_file to include with the message
    #[serde(default)]
    pub(super) attachment_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EmptyParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReadHistoryParams {
    /// The target to read history from — e.g. '#general', 'dm:@richard'
    pub(super) channel: String,
    /// Max number of messages to return (default 50, max 100)
    #[serde(default)]
    pub(super) limit: Option<u32>,
    /// Return messages before this seq number (for backward pagination)
    #[serde(default)]
    pub(super) before: Option<i64>,
    /// Return messages after this seq number (for catching up on unread)
    #[serde(default)]
    pub(super) after: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListTasksParams {
    /// The channel whose task board to view — e.g. '#engineering'
    pub(super) channel: String,
    /// Filter by status: all, todo, in_progress, in_review, done (default: all)
    #[serde(default)]
    pub(super) status: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TaskDef {
    /// Task title
    pub(super) title: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CreateTasksParams {
    /// The channel to create tasks in — e.g. '#engineering'
    pub(super) channel: String,
    /// Array of tasks to create
    pub(super) tasks: Vec<TaskDef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ClaimTasksParams {
    /// The channel whose tasks to claim — e.g. '#engineering'
    pub(super) channel: String,
    /// Task numbers to claim (e.g. [1, 3, 5])
    pub(super) task_numbers: Vec<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UnclaimTaskParams {
    /// The channel — e.g. '#engineering'
    pub(super) channel: String,
    /// The task number to unclaim (e.g. 3)
    pub(super) task_number: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UpdateTaskStatusParams {
    /// The channel — e.g. '#engineering'
    pub(super) channel: String,
    /// The task number to update (e.g. 3)
    pub(super) task_number: i64,
    /// The new status: todo, in_progress, in_review, or done
    pub(super) status: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UploadFileParams {
    /// Absolute path to the image file on your local filesystem
    pub(super) file_path: String,
    /// The channel target where this file will be used (e.g. '#general', 'dm:@richard')
    pub(super) channel: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ViewFileParams {
    /// The attachment UUID (from the 'id:...' shown in the message)
    pub(super) attachment_id: String,
}

// ---------------------------------------------------------------------------
// Decision Inbox — TRACE-ONLY scaffold
// ---------------------------------------------------------------------------
//
// Verifies the agent's proactive-dispatch behavior. The handler logs the
// payload and returns a synthetic decision_id; nothing persists. Storage,
// resume_with_prompt, and UI come back only after we see the agent emit
// this tool unprompted.

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct DecisionOptionParam {
    /// Short identifier (1-2 alphanumeric chars), e.g. "A", "B", "R1"
    pub(super) key: String,
    /// Short button label (≤40 chars)
    pub(super) label: String,
    /// Markdown body listing the consequences if the human picks this option (≤2048 chars)
    pub(super) body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CreateDecisionParams {
    /// One-line headline carrying the category and subject (≤80 chars)
    pub(super) headline: String,
    /// The actual ask in one sentence (≤120 chars)
    pub(super) question: String,
    /// 2..=6 options the human picks between
    pub(super) options: Vec<DecisionOptionParam>,
    /// Must equal one option's `key`. Always recommend; do not abstain.
    pub(super) recommended_key: String,
    /// Markdown context body (≤4096 chars). Suggested H2 sections: Why now, Evidence, Risk, Pressure, History, Dep tree, Related.
    #[serde(default)]
    pub(super) context: Option<String>,
}
