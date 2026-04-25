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
    /// The new status. The state machine is forward-only:
    ///   proposed -> todo (acceptance) | dismissed (rejection)
    ///   todo -> in_progress
    ///   in_progress -> in_review
    ///   in_review -> done
    /// No reverse edges in v1; reverts go via unclaim or a fresh task.
    pub(super) status: String,
}

/// Parameters for `propose_task`: agent proposes a task tied to a chat
/// message. The server snapshots the source (sender, content, created_at)
/// onto the task row so provenance survives source-message deletion.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ProposeTaskParams {
    /// The channel where the source message lives — e.g. '#engineering'
    pub(super) channel: String,
    /// Free-form title of the proposed task
    pub(super) title: String,
    /// UUID of the chat message that sparked the proposal. Must belong to
    /// the same channel; the server returns an error otherwise.
    pub(super) source_message_id: String,
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
