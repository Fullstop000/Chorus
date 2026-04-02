/** Re-exports: canonical definitions live in each feature `types.ts` beside that module. */

export type {
  ChannelInfo,
  HumanInfo,
  ChannelMemberInfo,
  ChannelMembersResponse,
  ResolveChannelResponse,
  Team,
  TeamMember,
  TeamResponse,
  ServerInfo,
  WhoamiResponse,
} from '../components/channels/types'

export type {
  AgentInfo,
  AgentEnvVar,
  RuntimeAuthStatus,
  RuntimeStatusInfo,
  AgentDetailResponse,
  ActivityMessage,
  ActivityResponse,
  ActivityEntryKind,
  ActivityEntry,
  ActivityLogEntry,
  ActivityLogResponse,
  WorkspaceResponse,
  WorkspaceFileResponse,
} from '../components/agents/types'

export type {
  AttachmentRef,
  ForwardedFrom,
  HistoryMessage,
  HistoryResponse,
  StreamEvent,
  UploadResponse,
  Target,
} from '../components/chat/types'

export type { TaskStatus, TaskInfo, TasksResponse } from '../components/tasks/types'

export type {
  InboxConversationState,
  InboxResponse,
  ConversationInboxRefreshResponse,
  ThreadInboxEntry,
  ThreadInboxResponse,
} from '../inbox/types'

export type { RealtimeMessage } from '../transport/types'
