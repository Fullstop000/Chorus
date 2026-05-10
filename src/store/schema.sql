-- Schema for Chorus store

-- Workspaces are the root collaboration boundary. Local-only deployments still
-- create an explicit workspace during setup.
CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    mode TEXT NOT NULL DEFAULT 'local_only',
    created_by_human_id TEXT REFERENCES humans(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workspace_members (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    human_id TEXT NOT NULL REFERENCES humans(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (workspace_id, human_id)
);

CREATE TABLE IF NOT EXISTS local_workspace_state (
    key TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE
);

-- Channels represent chat rooms, direct messages, system broadcasts, or task sub-channels.
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY, -- Unique UUID for the channel
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name TEXT NOT NULL, -- Human-readable name unique within a workspace
    description TEXT, -- Optional topic or description for the channel
    channel_type TEXT NOT NULL DEFAULT 'channel', -- Type of channel: 'channel' | 'dm' | 'system' | 'team' | 'task'
    archived INTEGER NOT NULL DEFAULT 0, -- 1 if archived, 0 if active
    created_at TEXT NOT NULL DEFAULT (datetime('now')), -- Timestamp of creation
    parent_channel_id TEXT REFERENCES channels(id) -- Parent channel for task sub-channels; NULL for all other types.
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_channels_workspace_name
    ON channels(workspace_id, name);

-- Memberships linking users/agents to channels.
CREATE TABLE IF NOT EXISTS channel_members (
    channel_id TEXT NOT NULL, -- Foreign key to channels.id
    member_id TEXT NOT NULL, -- ID of the member (human or agent)
    member_type TEXT NOT NULL, -- Type of member: 'human' or 'agent'
    last_read_seq INTEGER NOT NULL DEFAULT 0, -- The highest message sequence number read by this member
    PRIMARY KEY (channel_id, member_type, member_id)
);

-- Read state for the conversation inbox.
CREATE TABLE IF NOT EXISTS inbox_read_state (
    conversation_id TEXT NOT NULL, -- ID of the conversation/channel
    member_id TEXT NOT NULL, -- ID of the member
    member_type TEXT NOT NULL, -- Type of member
    last_read_seq INTEGER NOT NULL DEFAULT 0, -- Highest read sequence
    last_read_message_id TEXT, -- ID of the last read message
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the read state was last updated
    PRIMARY KEY (conversation_id, member_type, member_id)
);

-- Chat messages.
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY, -- Unique UUID for the message
    channel_id TEXT NOT NULL, -- ID of the channel where the message was sent
    sender_id TEXT NOT NULL, -- ID of the sender
    sender_type TEXT NOT NULL, -- Type of sender: 'human', 'agent', or 'system'
    sender_deleted INTEGER NOT NULL DEFAULT 0, -- 1 if deleted by the sender, 0 otherwise
    content TEXT NOT NULL, -- The actual text content of the message
    created_at TEXT NOT NULL DEFAULT (datetime('now')), -- Timestamp of creation
    seq INTEGER NOT NULL, -- Monotonically increasing sequence number within the channel
    forwarded_from TEXT, -- Optional JSON or text indicating where the message was forwarded from
    run_id TEXT, -- Telescope trace run id linking to trace_events
    trace_summary TEXT, -- JSON summary of the trace run (toolCalls, duration, status, categories)
    payload TEXT, -- Structured JSON for kind-discriminated message variants. Always paired with a human-readable `content` fallback. `audience` field controls agent visibility ("humans" hides from agents; absent = visible to all).
    UNIQUE(channel_id, seq)
);

-- Links between messages and attachments.
CREATE TABLE IF NOT EXISTS message_attachments (
    message_id TEXT NOT NULL, -- Foreign key to messages.id
    attachment_id TEXT NOT NULL, -- Foreign key to attachments.id
    PRIMARY KEY (message_id, attachment_id)
);

-- Trace events persisted for Telescope history.
CREATE TABLE IF NOT EXISTS trace_events (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL,     -- Links to messages.run_id
    seq INTEGER NOT NULL,     -- Monotonic within run
    timestamp_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,        -- Event type: thinking, tool_call, tool_result, text, turn_end, error
    data TEXT NOT NULL,        -- JSON payload for the event
    UNIQUE(run_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_trace_events_run_seq ON trace_events(run_id, seq);

-- AI Agents configuration and status.
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY, -- Unique UUID for the agent
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name TEXT UNIQUE NOT NULL, -- Unique handle within a workspace. Globally unique today; workspace scoping is enforced by idx_agents_workspace_name and may relax this in the future.
    display_name TEXT NOT NULL, -- Human-readable display name
    description TEXT, -- Description of the agent's role/capabilities
    system_prompt TEXT, -- Full system prompt for the LLM (templates inject rich prompts here)
    runtime TEXT NOT NULL, -- The runtime driver used (e.g., 'claude', 'codex')
    model TEXT NOT NULL, -- The specific LLM model used
    reasoning_effort TEXT, -- The reasoning effort configuration
    machine_id TEXT, -- Owner. Some(machine_id) = bound to that bridge; NULL = platform-local. Every agent has one owner.
    paused INTEGER NOT NULL DEFAULT 0, -- Soft-stop: 1 = bridge client should keep this agent stopped even though it's still in the desired set; 0 = run normally. Set by handle_agent_stop, cleared by handle_agent_start.
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the agent was created
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_workspace_name
    ON agents(workspace_id, name);

-- Environment variables for agents. Keyed by `agent_id` so a rename (which
-- changes `agents.name`) does not require touching this table.
CREATE TABLE IF NOT EXISTS agent_env_vars (
    agent_id TEXT NOT NULL, -- Foreign key to agents.id
    key TEXT NOT NULL, -- Environment variable key
    value TEXT NOT NULL, -- Environment variable value
    position INTEGER NOT NULL, -- Ordering position
    PRIMARY KEY (agent_id, key),
    FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
);

-- Human users.
CREATE TABLE IF NOT EXISTS humans (
    id TEXT PRIMARY KEY, -- Stable human identity
    name TEXT NOT NULL UNIQUE, -- Unique user-facing name
    auth_provider TEXT NOT NULL DEFAULT 'local', -- Future auth provider marker
    email TEXT UNIQUE, -- Future cloud auth email
    disabled_at TEXT, -- Future account disable timestamp
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the user was created
);

-- Tasks tracked within channels.
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY, -- Unique UUID for the task
    channel_id TEXT NOT NULL, -- Channel where the task is tracked
    task_number INTEGER NOT NULL, -- Sequential task number within the channel
    title TEXT NOT NULL, -- Title/summary of the task
    status TEXT NOT NULL DEFAULT 'todo', -- Current status (e.g., 'todo', 'in_progress', 'done')
    claimed_by_id TEXT, -- Optional user/agent ID who claimed the task
    claimed_by_type TEXT, -- Type of claimer: 'human' or 'agent'
    created_by_id TEXT NOT NULL, -- User/agent ID who created the task
    created_by_type TEXT NOT NULL, -- Type of creator: 'human' or 'agent'
    created_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the task was created
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the task was last updated
    sub_channel_id TEXT REFERENCES channels(id), -- Child channel owned by this task (ChannelType::Task)
    UNIQUE(channel_id, task_number)
);

-- Uploaded files and attachments.
CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY, -- Unique UUID for the attachment
    filename TEXT NOT NULL, -- Original filename
    mime_type TEXT NOT NULL, -- MIME type of the file
    size_bytes INTEGER NOT NULL, -- File size in bytes
    stored_path TEXT NOT NULL, -- Path where the file is stored on disk
    uploaded_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the file was uploaded
);

-- Teams of agents.
CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY, -- Unique UUID for the team
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name TEXT NOT NULL, -- Unique machine name for the team within a workspace
    display_name TEXT NOT NULL, -- Human-readable display name
    collaboration_model TEXT NOT NULL, -- Collaboration model used by the team
    leader_agent_name TEXT, -- Optional name of the leader agent
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the team was created
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_teams_workspace_name
    ON teams(workspace_id, name);

-- Memberships within teams.
CREATE TABLE IF NOT EXISTS team_members (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE, -- Foreign key to teams.id
    member_id TEXT NOT NULL, -- ID of the member
    member_type TEXT NOT NULL, -- Type of member (e.g., 'agent')
    role TEXT NOT NULL, -- Role within the team (e.g., 'leader', 'member')
    joined_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the member joined
    PRIMARY KEY (team_id, member_type, member_id)
);

-- Views

-- Explicit conversation history read model aligned with the current backing tables.
DROP VIEW IF EXISTS conversation_messages_view;
CREATE VIEW conversation_messages_view AS
SELECT
    m.id AS message_id,
    m.channel_id AS conversation_id,
    c.name AS conversation_name,
    c.channel_type AS conversation_type,
    m.sender_id AS sender_id,
    COALESCE(h.name, a.name, m.sender_id) AS sender_name,
    m.sender_type AS sender_type,
    m.sender_deleted AS sender_deleted,
    m.content AS content,
    m.created_at AS created_at,
    m.seq AS seq,
    m.forwarded_from AS forwarded_from,
    m.run_id AS run_id,
    m.trace_summary AS trace_summary,
    m.payload AS payload
FROM messages m
JOIN channels c ON c.id = m.channel_id
LEFT JOIN humans h ON m.sender_type = 'human' AND h.id = m.sender_id
LEFT JOIN agents a ON m.sender_type = 'agent' AND a.id = m.sender_id;

-- Inbox conversation state view
DROP VIEW IF EXISTS inbox_conversation_state_view;
CREATE VIEW inbox_conversation_state_view AS
SELECT
    cm.channel_id AS conversation_id,
    c.name AS conversation_name,
    c.channel_type AS conversation_type,
    cm.member_id AS member_id,
    COALESCE(h.name, a.name, cm.member_id) AS member_name,
    cm.member_type AS member_type,
    COALESCE(irs.last_read_seq, 0) AS last_read_seq,
    irs.last_read_message_id AS last_read_message_id,
    -- Channel-level unread count (excludes system-authored messages, which are
    -- ambient markers rather than unread signal).
    (
        SELECT COUNT(*)
        FROM messages m
        WHERE m.channel_id = cm.channel_id
          AND m.seq > COALESCE(irs.last_read_seq, 0)
          AND m.sender_type != 'system'
          AND NOT (
                        m.sender_id = cm.member_id
            AND m.sender_type = cm.member_type
          )
    ) AS unread_count
FROM channel_members cm
JOIN channels c ON c.id = cm.channel_id
LEFT JOIN humans h ON cm.member_type = 'human' AND h.id = cm.member_id
LEFT JOIN agents a ON cm.member_type = 'agent' AND a.id = cm.member_id
LEFT JOIN inbox_read_state irs
  ON irs.conversation_id = cm.channel_id
 AND irs.member_type = cm.member_type
 AND irs.member_id = cm.member_id;
-- Note: archived task sub-channels still appear in this view so the
-- per-conversation read-cursor + notification lookup can find them when the
-- user is viewing an archived sub-channel via the task detail page. The
-- sidebar LIST query (`get_inbox_conversation_notifications`) filters them
-- out instead — see that method for the WHERE clause.

-- Sessions held by an agent. One row per (agent, runtime-assigned session id).
-- `is_active` marks the session that should be resumed on next start.
CREATE TABLE IF NOT EXISTS agent_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    runtime TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(agent_id, session_id)
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent_active
    ON agent_sessions(agent_id, is_active);

-- Decisions emitted by agents via dispatch_decision MCP tool.
-- One row per emission. Status transitions Open → Resolved are CAS-protected
-- (UPDATE WHERE status='open') so two simultaneous picks can't both succeed.
-- Stores the full payload as JSON in TEXT (SQLite has no JSONB); shape is
-- enforced at the validator boundary, not the schema.
CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    status TEXT NOT NULL CHECK(status IN ('open','resolved')),
    payload_json TEXT NOT NULL,
    picked_key TEXT,
    picked_note TEXT,
    resolved_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_decisions_workspace_status
    ON decisions(workspace_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_decisions_agent_status
    ON decisions(agent_id, status);
