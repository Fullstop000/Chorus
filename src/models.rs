use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Channel ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: ChannelType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Channel,
    Dm,
    /// System-managed channels (e.g. #shared-memory). Not listed in the UI channel list.
    /// Agents may not post to them directly via send_message.
    System,
}

// ── Shared knowledge (group memory store) ──

/// A single entry in the shared knowledge store, written by one agent and readable by all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    /// Short label describing the fact, e.g. "rate-limiting approach"
    pub key: String,
    /// The content of the fact
    pub value: String,
    /// Space-separated tags used for filtering, e.g. "research task-42"
    pub tags: String,
    pub author_agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_context: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RememberRequest {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "channelContext")]
    pub channel_context: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RememberResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct RecallQuery {
    pub query: Option<String>,
    pub tags: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecallResponse {
    pub entries: Vec<KnowledgeEntry>,
}

// ── Message ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub thread_parent_id: Option<String>,
    pub sender_name: String,
    pub sender_type: SenderType,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub seq: i64,
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderType {
    Human,
    Agent,
}

// ── Task ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub channel_id: String,
    pub task_number: i64,
    pub title: String,
    pub status: TaskStatus,
    pub claimed_by: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    InReview,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }

    pub fn from_status_str(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "in_review" => Some(Self::InReview),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn can_transition_to(&self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Todo, Self::InProgress)
                | (Self::InProgress, Self::InReview)
                | (Self::InProgress, Self::Done)
                | (Self::InReview, Self::Done)
                | (Self::InReview, Self::InProgress)
        )
    }
}

// ── Agent ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub env_vars: Vec<AgentEnvVar>,
    pub status: AgentStatus,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEnvVar {
    pub key: String,
    pub value: String,
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Active,
    Sleeping,
    Inactive,
}

// ── Human ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    pub name: String,
    pub created_at: DateTime<Utc>,
}

// ── Attachment ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub stored_path: String,
    pub uploaded_at: DateTime<Utc>,
}

// ── Channel membership ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    pub channel_id: String,
    pub member_name: String,
    pub member_type: SenderType,
    pub last_read_seq: i64,
}

// ── Agent config (for starting agents) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub session_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub env_vars: Vec<AgentEnvVar>,
}

// ── API request/response types ──

#[derive(Debug, Serialize, Deserialize)]
pub struct SendRequest {
    pub target: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "attachmentIds")]
    pub attachment_ids: Vec<String>,
    /// Skip fan-out to other agents when the caller wants a human-only side effect,
    /// such as "send this message and create one task" without triggering agent replies.
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendResponse {
    #[serde(rename = "messageId")]
    pub message_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReceiveResponse {
    pub messages: Vec<ReceivedMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedMessage {
    pub message_id: String,
    pub channel_name: String,
    pub channel_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_type: Option<String>,
    pub sender_name: String,
    pub sender_type: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub id: String,
    pub filename: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub messages: Vec<HistoryMessage>,
    pub has_more: bool,
    pub last_read_seq: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub id: String,
    pub seq: i64,
    pub content: String,
    #[serde(rename = "senderName")]
    pub sender_name: String,
    #[serde(rename = "senderType")]
    pub sender_type: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "senderDeleted")]
    pub sender_deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    #[serde(rename = "replyCount", skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityMessage {
    pub id: String,
    pub seq: i64,
    pub content: String,
    #[serde(rename = "channelName")]
    pub channel_name: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub channels: Vec<ChannelInfo>,
    /// System-managed channels (e.g. #shared-memory). Excluded from the regular channel list.
    pub system_channels: Vec<ChannelInfo>,
    pub agents: Vec<AgentInfo>,
    pub humans: Vec<HumanInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChannelInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub joined: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(rename = "reasoningEffort", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Live activity state: online | thinking | working | offline
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_detail: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentDetailResponse {
    pub agent: AgentInfo,
    #[serde(rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentEnvVarPayload {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HumanInfo {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskInfo {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    pub title: String,
    pub status: String,
    #[serde(rename = "claimedByName")]
    pub claimed_by_name: Option<String>,
    #[serde(rename = "createdByName")]
    pub created_by_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTasksRequest {
    pub channel: String,
    pub tasks: Vec<CreateTaskItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTaskItem {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimTasksRequest {
    pub channel: String,
    pub task_numbers: Vec<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResult {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskStatusRequest {
    pub channel: String,
    pub task_number: i64,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnclaimTaskRequest {
    pub channel: String,
    pub task_number: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveChannelRequest {
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveChannelResponse {
    #[serde(rename = "channelId")]
    pub channel_id: String,
}

// ── Activity log (in-memory living log per agent) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEntry {
    Thinking {
        text: String,
    },
    ToolStart {
        tool_name: String,
        tool_input: String,
    },
    Text {
        text: String,
    },
    MessageReceived {
        channel_label: String,
        sender_name: String,
        content: String,
    },
    MessageSent {
        target: String,
        content: String,
    },
    Status {
        activity: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub entry: ActivityEntry,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityLogResponse {
    pub entries: Vec<ActivityLogEntry>,
    pub agent_activity: String,
    pub agent_detail: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentRequest {
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RestartAgentRequest {
    pub mode: RestartMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    Restart,
    ResetSession,
    FullReset,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteAgentRequest {
    pub mode: DeleteMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    PreserveWorkspace,
    DeleteWorkspace,
}

fn default_runtime() -> String {
    "claude".to_string()
}

fn default_model() -> String {
    "sonnet".to_string()
}
