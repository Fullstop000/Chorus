import type {
  AgentInfo,
  ChannelInfo,
  InboxConversationState,
  StreamEvent,
  ThreadInboxEntry,
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

export function createInboxState(): InboxState {
  return {
    conversations: {},
    threads: {},
  }
}

export function threadNotificationKey(conversationId: string, threadParentId: string): string {
  return `${conversationId}:${threadParentId}`
}

export function conversationThreadUnreadCount(
  state: InboxState,
  conversationId?: string | null
): number {
  if (!conversationId) return 0
  let unreadCount = 0
  for (const threadState of Object.values(state.threads)) {
    if (threadState.conversationId !== conversationId) continue
    unreadCount += threadState.unreadCount
  }
  return unreadCount
}

export function mergeChannelThreadInboxEntries(
  entries: ThreadInboxEntry[],
  state: InboxState,
  conversationId?: string | null
): ThreadInboxEntry[] {
  const merged = entries
    .filter((entry) => !conversationId || entry.conversationId === conversationId)
    .map((entry) => {
      const liveState = state.threads[threadNotificationKey(entry.conversationId, entry.threadParentId)]
      if (!liveState) return entry
      return {
        ...entry,
        latestSeq: liveState.latestSeq,
        lastReadSeq: liveState.lastReadSeq,
        unreadCount: liveState.unreadCount,
        lastReplyMessageId: liveState.lastReplyMessageId ?? entry.lastReplyMessageId ?? null,
        lastReplyAt: liveState.lastReplyAt ?? entry.lastReplyAt ?? null,
      }
    })

  merged.sort((left, right) =>
    (right.latestSeq - left.latestSeq) ||
    (right.parentSeq - left.parentSeq)
  )

  return merged
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

function messageIdFromEvent(event: StreamEvent): string | null {
  const value = event.payload.messageId
  return typeof value === 'string' && value.length > 0 ? value : null
}

export function applyInboxEvent(state: InboxState, event: StreamEvent): InboxState {
  if (event.eventType !== 'message.created') {
    return state
  }

  const current = state.conversations[event.channelId]
  if (!current) {
    return state
  }

  const latestSeq = Math.max(current.latestSeq, event.latestSeq)
  const unreadCount = Math.max(latestSeq - current.lastReadSeq, 0)
  const lastMessageId = messageIdFromEvent(event) ?? current.lastMessageId ?? null

  if (
    latestSeq === current.latestSeq &&
    unreadCount === current.unreadCount &&
    lastMessageId === (current.lastMessageId ?? null)
  ) {
    return state
  }

  return {
    ...state,
    conversations: {
      ...state.conversations,
      [event.channelId]: {
        ...current,
        latestSeq,
        unreadCount,
        lastMessageId,
      },
    },
  }
}

export function applyConversationRead(
  state: InboxState,
  conversationId: string,
  lastReadSeq: number
): InboxState {
  const current = state.conversations[conversationId]
  if (!current || lastReadSeq <= current.lastReadSeq) {
    return state
  }

  const nextLastReadSeq = Math.max(current.lastReadSeq, lastReadSeq)
  const unreadCount = Math.max(current.latestSeq - nextLastReadSeq, 0)

  return {
    ...state,
    conversations: {
      ...state.conversations,
      [conversationId]: {
        ...current,
        lastReadSeq: nextLastReadSeq,
        unreadCount,
      },
    },
  }
}

export function bootstrapInboxState(
  conversations: InboxConversationState[],
  channels: ChannelInfo[] = []
): InboxState {
  const nextState = createInboxState()
  for (const conversation of conversations) {
    nextState.conversations[conversation.conversationId] = conversation
  }
  return ensureInboxConversations(nextState, channels)
}

export function ensureInboxConversations(
  state: InboxState,
  channels: ChannelInfo[] = []
): InboxState {
  let nextState = state
  for (const channel of channels) {
    if (!channel.id || channel.joined === false) continue
    if (nextState.conversations[channel.id]) continue
    if (nextState === state) {
      nextState = {
        ...state,
        conversations: {
          ...state.conversations,
        },
      }
    }
    nextState.conversations[channel.id] = {
      conversationId: channel.id,
      conversationName: channel.name,
      conversationType: channel.channel_type ?? 'channel',
      latestSeq: 0,
      lastReadSeq: 0,
      unreadCount: 0,
      lastReadMessageId: null,
      lastMessageId: null,
      lastMessageAt: null,
    }
  }
  return nextState
}
