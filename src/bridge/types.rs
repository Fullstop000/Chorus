use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SendMessageParams {
    /// Where to send. Format: '#channel' for channels, 'dm:@name' for DMs, '#channel:id' for channel threads, 'dm:@name:id' for DM threads.
    pub(super) target: String,
    /// The message content
    pub(super) content: String,
    /// Optional attachment IDs from upload_file to include with the message
    #[serde(default)]
    pub(super) attachment_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReceiveMessageParams {
    /// Whether to block (wait) for new messages (default true)
    #[serde(default = "default_true")]
    pub(super) block: Option<bool>,
    /// How long to wait in ms when blocking (default 59000)
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
}

pub(super) fn default_true() -> Option<bool> {
    Some(true)
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

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RememberParams {
    /// Short label for this fact, e.g. "rate-limiting approach" or "api shape"
    pub(super) key: String,
    /// The full content of the fact
    pub(super) value: String,
    /// Optional space-separated tags for filtering later, e.g. "research task-42"
    #[serde(default)]
    pub(super) tags: Option<String>,
    /// Optional channel context where this fact was discovered (e.g. '#general')
    #[serde(default, rename = "channelContext")]
    pub(super) channel_context: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RecallParams {
    /// Keyword query to search across all stored facts (key, value, and tags)
    #[serde(default)]
    pub(super) query: Option<String>,
    /// Space-separated tags to filter by (all listed tags must be present)
    #[serde(default)]
    pub(super) tags: Option<String>,
}
