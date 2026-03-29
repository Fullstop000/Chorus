import type {
  AgentInfo,
  ChannelInfo,
  ConversationStatePayload,
  InboxConversationState,
  RealtimeEvent,
  ThreadStatePayload,
} from './types'

export interface ThreadInboxState {
  conversationId: string
  threadParentId: string
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  lastReadMessageId?: string | null
  lastReplyMessageId?: string | null
  lastReplyAt?: string | null
}

export interface InboxState {
  conversations: Record<string, InboxConversationState>
  threads: Record<string, ThreadInboxState>
}

export interface ConversationRegistryEntry {
  conversationId: string
  target: string
  conversationType: string
  label: string
}

interface BuildConversationRegistryOptions {
  currentUser: string
  systemChannels: ChannelInfo[]
  channels: ChannelInfo[]
  dmChannels: ChannelInfo[]
  agents: AgentInfo[]
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function asString(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null
}

function conversationIdFromEvent(event: RealtimeEvent): string | null {
  if (typeof event.payload.conversationId === 'string') {
    return event.payload.conversationId
  }
  if (typeof event.channelId === 'string') {
    return event.channelId
  }
  if (typeof event.streamId === 'string' && event.streamId.startsWith('conversation:')) {
    return event.streamId.slice('conversation:'.length)
  }
  return null
}

function threadParentIdFromEvent(event: RealtimeEvent): string | null {
  return asString(event.threadParentId) ?? asString(event.payload.threadParentId)
}

function normalizeConversationState(
  prior: InboxConversationState | undefined,
  payload: Record<string, unknown>
): InboxConversationState | null {
  const conversationId = asString(payload.conversationId) ?? prior?.conversationId ?? null
  if (!conversationId) return null

  return {
    conversationId,
    conversationName: asString(payload.conversationName) ?? prior?.conversationName ?? conversationId,
    conversationType: asString(payload.conversationType) ?? prior?.conversationType ?? 'channel',
    latestSeq: asNumber(payload.latestSeq) ?? prior?.latestSeq ?? 0,
    lastReadSeq: asNumber(payload.lastReadSeq) ?? prior?.lastReadSeq ?? 0,
    unreadCount: asNumber(payload.unreadCount) ?? prior?.unreadCount ?? 0,
    lastReadMessageId:
      asString(payload.lastReadMessageId) ?? prior?.lastReadMessageId ?? null,
    lastMessageId:
      asString(payload.lastMessageId) ??
      asString(payload.messageId) ??
      prior?.lastMessageId ??
      null,
    lastMessageAt:
      asString(payload.lastMessageAt) ??
      asString(payload.createdAt) ??
      prior?.lastMessageAt ??
      null,
  }
}

function normalizeThreadState(
  prior: ThreadInboxState | undefined,
  payload: Record<string, unknown>
): ThreadInboxState | null {
  const conversationId = asString(payload.conversationId) ?? prior?.conversationId ?? null
  const threadParentId = asString(payload.threadParentId) ?? prior?.threadParentId ?? null
  if (!conversationId || !threadParentId) return null

  return {
    conversationId,
    threadParentId,
    latestSeq: asNumber(payload.latestSeq) ?? prior?.latestSeq ?? 0,
    lastReadSeq: asNumber(payload.lastReadSeq) ?? prior?.lastReadSeq ?? 0,
    unreadCount: asNumber(payload.unreadCount) ?? prior?.unreadCount ?? 0,
    lastReadMessageId:
      asString(payload.lastReadMessageId) ?? prior?.lastReadMessageId ?? null,
    lastReplyMessageId:
      asString(payload.lastReplyMessageId) ??
      asString(payload.messageId) ??
      prior?.lastReplyMessageId ??
      null,
    lastReplyAt:
      asString(payload.lastReplyAt) ??
      asString(payload.createdAt) ??
      prior?.lastReplyAt ??
      null,
  }
}

export function createInboxState(): InboxState {
  return {
    conversations: {},
    threads: {},
  }
}

export function threadNotificationKey(conversationId: string, threadParentId: string): string {
  return `${conversationId}:${threadParentId}`
}

export function dmConversationNameForParticipants(left: string, right: string): string {
  return `dm-${[left, right].sort().join('-')}`
}

function dmPeerName(currentUser: string, dmChannelName: string): string | null {
  if (!dmChannelName.startsWith('dm-')) return null
  const participants = dmChannelName.slice(3).split('-')
  if (participants.length < 2) return null
  return participants.find((participant) => participant !== currentUser) ?? null
}

export function buildConversationRegistry(
  options: BuildConversationRegistryOptions
): ConversationRegistryEntry[] {
  const entries: ConversationRegistryEntry[] = []
  const seenConversationIds = new Set<string>()

  const pushEntry = (entry: ConversationRegistryEntry | null) => {
    if (!entry || seenConversationIds.has(entry.conversationId)) return
    seenConversationIds.add(entry.conversationId)
    entries.push(entry)
  }

  for (const channel of options.systemChannels) {
    if (!channel.id || channel.joined === false) continue
    pushEntry({
      conversationId: channel.id,
      target: `#${channel.name}`,
      conversationType: channel.channel_type ?? 'system',
      label: channel.name,
    })
  }

  for (const channel of options.channels) {
    if (!channel.id || channel.joined === false) continue
    pushEntry({
      conversationId: channel.id,
      target: `#${channel.name}`,
      conversationType: channel.channel_type ?? 'channel',
      label: channel.name,
    })
  }

  const knownAgents = new Set(options.agents.map((agent) => agent.name))
  for (const channel of options.dmChannels) {
    if (!channel.id || channel.joined === false) continue
    const peer = dmPeerName(options.currentUser, channel.name)
    if (!peer || !knownAgents.has(peer)) continue
    pushEntry({
      conversationId: channel.id,
      target: `dm:@${peer}`,
      conversationType: 'dm',
      label: peer,
    })
  }

  return entries
}

export function applyInboxEvent(state: InboxState, event: RealtimeEvent): InboxState {
  switch (event.eventType) {
    case 'conversation.state':
    case 'conversation.read_cursor_set': {
      const conversationId = conversationIdFromEvent(event)
      if (!conversationId) return state
      const nextConversation = normalizeConversationState(state.conversations[conversationId], {
        ...event.payload,
        conversationId,
      })
      if (!nextConversation) return state
      return {
        ...state,
        conversations: {
          ...state.conversations,
          [conversationId]: nextConversation,
        },
      }
    }
    case 'thread.state':
    case 'thread.read_cursor_set': {
      const conversationId = conversationIdFromEvent(event)
      const threadParentId = threadParentIdFromEvent(event)
      if (!conversationId || !threadParentId) return state
      const key = threadNotificationKey(conversationId, threadParentId)
      const nextThread = normalizeThreadState(state.threads[key], {
        ...event.payload,
        conversationId,
        threadParentId,
      })
      if (!nextThread) return state
      return {
        ...state,
        threads: {
          ...state.threads,
          [key]: nextThread,
        },
      }
    }
    default:
      return state
  }
}

export function bootstrapInboxState(
  conversations: InboxConversationState[]
): InboxState {
  const nextState = createInboxState()
  for (const conversation of conversations) {
    nextState.conversations[conversation.conversationId] = conversation
  }
  return nextState
}

export function conversationPayload(
  event: RealtimeEvent
): ConversationStatePayload | null {
  const conversationId = conversationIdFromEvent(event)
  if (!conversationId) return null
  return {
    conversationId,
    target: asString(event.payload.target) ?? undefined,
    latestSeq: asNumber(event.payload.latestSeq) ?? 0,
    lastReadSeq: asNumber(event.payload.lastReadSeq) ?? 0,
    unreadCount: asNumber(event.payload.unreadCount) ?? 0,
    lastMessageId:
      asString(event.payload.lastMessageId) ??
      asString(event.payload.messageId) ??
      undefined,
    lastMessageAt:
      asString(event.payload.lastMessageAt) ??
      asString(event.payload.createdAt) ??
      undefined,
    lastReadMessageId: asString(event.payload.lastReadMessageId) ?? undefined,
    conversationType: asString(event.payload.conversationType) ?? undefined,
    threadParentId: asString(event.payload.threadParentId) ?? undefined,
    messageId: asString(event.payload.messageId) ?? undefined,
  }
}

export function threadPayload(event: RealtimeEvent): ThreadStatePayload | null {
  const conversationId = conversationIdFromEvent(event)
  const threadParentId = threadParentIdFromEvent(event)
  if (!conversationId || !threadParentId) return null
  return {
    conversationId,
    threadParentId,
    latestSeq: asNumber(event.payload.latestSeq) ?? 0,
    lastReadSeq: asNumber(event.payload.lastReadSeq) ?? 0,
    unreadCount: asNumber(event.payload.unreadCount) ?? 0,
    lastReadMessageId: asString(event.payload.lastReadMessageId) ?? undefined,
    lastReplyMessageId:
      asString(event.payload.lastReplyMessageId) ??
      asString(event.payload.messageId) ??
      undefined,
    lastReplyAt:
      asString(event.payload.lastReplyAt) ??
      asString(event.payload.createdAt) ??
      undefined,
  }
}
