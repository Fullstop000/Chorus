-- Schema for Chorus store

-- Channels represent chat rooms, direct messages, or system broadcasts.
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY, -- Unique UUID for the channel
    name TEXT UNIQUE NOT NULL, -- Human-readable unique name (e.g., 'general', 'dm-alice-bob')
    description TEXT, -- Optional topic or description for the channel
    channel_type TEXT NOT NULL DEFAULT 'channel', -- Type of channel: 'channel', 'dm', or 'system'
    archived INTEGER NOT NULL DEFAULT 0, -- 1 if archived, 0 if active
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- Timestamp of creation
);

-- Memberships linking users/agents to channels.
CREATE TABLE IF NOT EXISTS channel_members (
    channel_id TEXT NOT NULL, -- Foreign key to channels.id
    member_name TEXT NOT NULL, -- Name of the member (human or agent)
    member_type TEXT NOT NULL, -- Type of member: 'human' or 'agent'
    last_read_seq INTEGER NOT NULL DEFAULT 0, -- The highest message sequence number read by this member
    PRIMARY KEY (channel_id, member_name)
);

-- Read state for top-level conversation inbox.
CREATE TABLE IF NOT EXISTS inbox_read_state (
    conversation_id TEXT NOT NULL, -- ID of the conversation/channel
    member_name TEXT NOT NULL, -- Name of the member
    member_type TEXT NOT NULL, -- Type of member
    last_read_seq INTEGER NOT NULL DEFAULT 0, -- Highest read sequence
    last_read_message_id TEXT, -- ID of the last read message
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the read state was last updated
    PRIMARY KEY (conversation_id, member_name)
);

-- Read state for specific threads within a conversation.
CREATE TABLE IF NOT EXISTS inbox_thread_read_state (
    conversation_id TEXT NOT NULL, -- ID of the conversation/channel
    thread_parent_id TEXT NOT NULL, -- ID of the parent message defining the thread
    member_name TEXT NOT NULL, -- Name of the member
    member_type TEXT NOT NULL, -- Type of member
    last_read_seq INTEGER NOT NULL DEFAULT 0, -- Highest read sequence in the thread
    last_read_message_id TEXT, -- ID of the last read message in the thread
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the read state was last updated
    PRIMARY KEY (conversation_id, thread_parent_id, member_name)
);

-- Chat messages.
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY, -- Unique UUID for the message
    channel_id TEXT NOT NULL, -- ID of the channel where the message was sent
    thread_parent_id TEXT, -- Optional ID of the parent message if this is a reply
    sender_name TEXT NOT NULL, -- Name of the sender
    sender_type TEXT NOT NULL, -- Type of sender: 'human', 'agent', or 'system'
    sender_deleted INTEGER NOT NULL DEFAULT 0, -- 1 if deleted by the sender, 0 otherwise
    content TEXT NOT NULL, -- The actual text content of the message
    created_at TEXT NOT NULL DEFAULT (datetime('now')), -- Timestamp of creation
    seq INTEGER NOT NULL, -- Monotonically increasing sequence number within the channel
    forwarded_from TEXT, -- Optional JSON or text indicating where the message was forwarded from
    UNIQUE(channel_id, seq)
);

-- Links between messages and attachments.
CREATE TABLE IF NOT EXISTS message_attachments (
    message_id TEXT NOT NULL, -- Foreign key to messages.id
    attachment_id TEXT NOT NULL, -- Foreign key to attachments.id
    PRIMARY KEY (message_id, attachment_id)
);

-- AI Agents configuration and status.
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY, -- Unique UUID for the agent
    name TEXT UNIQUE NOT NULL, -- Unique machine name
    display_name TEXT NOT NULL, -- Human-readable display name
    description TEXT, -- Description of the agent's role/capabilities
    system_prompt TEXT, -- Full system prompt for the LLM (templates inject rich prompts here)
    runtime TEXT NOT NULL, -- The runtime driver used (e.g., 'claude', 'codex')
    model TEXT NOT NULL, -- The specific LLM model used
    reasoning_effort TEXT, -- The reasoning effort configuration
    status TEXT NOT NULL DEFAULT 'inactive', -- Current status (e.g., 'active', 'inactive')
    session_id TEXT, -- Optional ID for the current session/process
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the agent was created
);

-- Environment variables for agents.
CREATE TABLE IF NOT EXISTS agent_env_vars (
    agent_name TEXT NOT NULL, -- Foreign key to agents.name
    key TEXT NOT NULL, -- Environment variable key
    value TEXT NOT NULL, -- Environment variable value
    position INTEGER NOT NULL, -- Ordering position
    PRIMARY KEY (agent_name, key),
    FOREIGN KEY (agent_name) REFERENCES agents(name) ON DELETE CASCADE
);

-- Human users.
CREATE TABLE IF NOT EXISTS humans (
    name TEXT PRIMARY KEY, -- Unique username
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the user was created
);

-- Tasks tracked within channels.
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY, -- Unique UUID for the task
    channel_id TEXT NOT NULL, -- Channel where the task is tracked
    task_number INTEGER NOT NULL, -- Sequential task number within the channel
    title TEXT NOT NULL, -- Title/summary of the task
    status TEXT NOT NULL DEFAULT 'todo', -- Current status (e.g., 'todo', 'in_progress', 'done')
    claimed_by TEXT, -- Optional user/agent who claimed the task
    created_by TEXT NOT NULL, -- User/agent who created the task
    created_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the task was created
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the task was last updated
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
    name TEXT UNIQUE NOT NULL, -- Unique machine name for the team
    display_name TEXT NOT NULL, -- Human-readable display name
    collaboration_model TEXT NOT NULL, -- Collaboration model used by the team
    leader_agent_name TEXT, -- Optional name of the leader agent
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the team was created
);

-- Memberships within teams.
CREATE TABLE IF NOT EXISTS team_members (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE, -- Foreign key to teams.id
    member_name TEXT NOT NULL, -- Name of the member
    member_type TEXT NOT NULL, -- Type of member (e.g., 'agent')
    member_id TEXT NOT NULL, -- ID of the member
    role TEXT NOT NULL, -- Role within the team (e.g., 'leader', 'member')
    joined_at TEXT NOT NULL DEFAULT (datetime('now')), -- When the member joined
    PRIMARY KEY (team_id, member_name)
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
    m.thread_parent_id AS thread_parent_id,
    m.sender_name AS sender_name,
    m.sender_type AS sender_type,
    m.sender_deleted AS sender_deleted,
    m.content AS content,
    m.created_at AS created_at,
    m.seq AS seq,
    m.forwarded_from AS forwarded_from
FROM messages m
JOIN channels c ON c.id = m.channel_id;

-- Thread summary reads projection view.
DROP VIEW IF EXISTS thread_summaries_view;
CREATE VIEW thread_summaries_view AS
SELECT
    parent.channel_id AS conversation_id,
    parent.id AS parent_message_id,
    COUNT(reply.id) AS reply_count,
    (
        SELECT reply_last.id
        FROM messages reply_last
        WHERE reply_last.channel_id = parent.channel_id
          AND reply_last.thread_parent_id = parent.id
        ORDER BY reply_last.seq DESC
        LIMIT 1
    ) AS last_reply_message_id,
    (
        SELECT reply_last.created_at
        FROM messages reply_last
        WHERE reply_last.channel_id = parent.channel_id
          AND reply_last.thread_parent_id = parent.id
        ORDER BY reply_last.seq DESC
        LIMIT 1
    ) AS last_reply_at,
    (
        SELECT COUNT(*)
        FROM (
            SELECT parent.sender_name AS participant_name
            UNION
            SELECT reply_participant.sender_name
            FROM messages reply_participant
            WHERE reply_participant.channel_id = parent.channel_id
              AND reply_participant.thread_parent_id = parent.id
        )
    ) AS participant_count
FROM messages parent
LEFT JOIN messages reply
  ON reply.channel_id = parent.channel_id
 AND reply.thread_parent_id = parent.id
WHERE parent.thread_parent_id IS NULL
GROUP BY parent.channel_id, parent.id;

-- Inbox conversation state view
DROP VIEW IF EXISTS inbox_conversation_state_view;
CREATE VIEW inbox_conversation_state_view AS
SELECT
    cm.channel_id AS conversation_id,
    c.name AS conversation_name,
    c.channel_type AS conversation_type,
    cm.member_name AS member_name,
    cm.member_type AS member_type,
    COALESCE(irs.last_read_seq, 0) AS last_read_seq,
    irs.last_read_message_id AS last_read_message_id,
    -- Channel-level unread count (top-level messages only, excludes thread replies
    -- and system-authored messages, which are ambient markers, not unread signal).
    (
        SELECT COUNT(*)
        FROM messages top_level
        WHERE top_level.channel_id = cm.channel_id
          AND top_level.thread_parent_id IS NULL
          AND top_level.seq > COALESCE(irs.last_read_seq, 0)
          AND top_level.sender_type != 'system'
          AND NOT (
            top_level.sender_name = cm.member_name
            AND top_level.sender_type = cm.member_type
          )
    ) AS unread_count,
    -- Thread-level unread count (all accessible thread replies, shown in thread tab;
    -- excludes system-authored replies for the same reason).
    (
        SELECT COUNT(*)
        FROM messages reply
        LEFT JOIN inbox_thread_read_state itrs
          ON itrs.conversation_id = reply.channel_id
         AND itrs.thread_parent_id = reply.thread_parent_id
         AND itrs.member_name = cm.member_name
        WHERE reply.channel_id = cm.channel_id
          AND reply.thread_parent_id IS NOT NULL
          AND reply.seq > COALESCE(itrs.last_read_seq, 0)
          AND reply.sender_type != 'system'
          AND NOT (
            reply.sender_name = cm.member_name
            AND reply.sender_type = cm.member_type
          )
          AND (
            cm.member_type != 'agent'
            OR EXISTS (
                SELECT 1
                FROM messages parent
                WHERE parent.id = reply.thread_parent_id
                  AND parent.channel_id = cm.channel_id
                  AND parent.sender_type = 'agent'
                  AND parent.sender_name = cm.member_name
            )
            OR EXISTS (
                SELECT 1
                FROM messages prior
                WHERE prior.channel_id = cm.channel_id
                  AND prior.thread_parent_id = reply.thread_parent_id
                  AND prior.sender_type = 'agent'
                  AND prior.sender_name = cm.member_name
                  AND prior.seq < reply.seq
            )
          )
    ) AS thread_unread_count
FROM channel_members cm
JOIN channels c ON c.id = cm.channel_id
LEFT JOIN inbox_read_state irs
  ON irs.conversation_id = cm.channel_id
 AND irs.member_name = cm.member_name;
