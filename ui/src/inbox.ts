import type {
  AgentInfo,
  ChannelInfo,
  ConversationInboxRefreshResponse,
  InboxConversationState,
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

/** Authoritative inbox fields returned from `POST .../read-cursor`. */
export interface ReadCursorAckPayload {
  conversationId: string
  conversationUnreadCount: number
  conversationLastReadSeq: number
  conversationLatestSeq: number
  conversationThreadUnreadCount?: number
  threadParentId?: string | null
  threadUnreadCount?: number
  threadLastReadSeq?: number
  threadLatestSeq?: number
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

/** Merge GET /inbox-notification (after message.created or explicit refresh). */
export function mergeInboxNotificationRefresh(
  state: InboxState,
  payload: ConversationInboxRefreshResponse
): InboxState {
  const id = payload.conversation.conversationId
  const live = state.conversations[id]
  if (live && payload.conversation.latestSeq < live.latestSeq) {
    return state
  }

  const mergedConv: InboxConversationState = live
    ? {
        ...live,
        ...payload.conversation,
        lastReadMessageId:
          payload.conversation.lastReadMessageId ?? live.lastReadMessageId ?? null,
      }
    : payload.conversation

  let threads = state.threads
  if (payload.thread) {
    const key = threadNotificationKey(
      payload.thread.conversationId,
      payload.thread.threadParentId
    )
    const prior = state.threads[key]
    threads = {
      ...state.threads,
      [key]: {
        conversationId: payload.thread.conversationId,
        threadParentId: payload.thread.threadParentId,
        latestSeq: payload.thread.latestSeq,
        lastReadSeq: payload.thread.lastReadSeq,
        unreadCount: payload.thread.unreadCount,
        lastReadMessageId: prior?.lastReadMessageId,
        lastReplyMessageId:
          payload.thread.lastReplyMessageId ?? prior?.lastReplyMessageId ?? null,
        lastReplyAt: payload.thread.lastReplyAt ?? prior?.lastReplyAt ?? null,
      },
    }
  }

  return {
    ...state,
    conversations: {
      ...state.conversations,
      [id]: mergedConv,
    },
    threads,
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

/** Apply server read-cursor response so sidebar/thread badges match SQLite inbox views. */
export function mergeReadCursorAckIntoInboxState(
  state: InboxState,
  ack: ReadCursorAckPayload
): InboxState {
  const current = state.conversations[ack.conversationId]
  if (!current) {
    return state
  }

  const nextConversation: InboxConversationState = {
    ...current,
    unreadCount: ack.conversationUnreadCount,
    threadUnreadCount: ack.conversationThreadUnreadCount ?? current.threadUnreadCount,
    lastReadSeq: ack.conversationLastReadSeq,
    latestSeq: ack.conversationLatestSeq,
  }

  const hasThreadSnapshot =
    ack.threadParentId &&
    ack.threadUnreadCount != null &&
    ack.threadLastReadSeq != null &&
    ack.threadLatestSeq != null

  if (!hasThreadSnapshot) {
    return {
      ...state,
      conversations: {
        ...state.conversations,
        [ack.conversationId]: nextConversation,
      },
    }
  }

  const key = threadNotificationKey(ack.conversationId, ack.threadParentId!)
  const priorThread = state.threads[key]

  return {
    ...state,
    conversations: {
      ...state.conversations,
      [ack.conversationId]: nextConversation,
    },
    threads: {
      ...state.threads,
      [key]: {
        conversationId: ack.conversationId,
        threadParentId: ack.threadParentId!,
        latestSeq: ack.threadLatestSeq!,
        lastReadSeq: ack.threadLastReadSeq!,
        unreadCount: ack.threadUnreadCount!,
        lastReadMessageId: priorThread?.lastReadMessageId,
        lastReplyMessageId: priorThread?.lastReplyMessageId,
        lastReplyAt: priorThread?.lastReplyAt,
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
      threadUnreadCount: 0,
      lastReadMessageId: null,
      lastMessageId: null,
      lastMessageAt: null,
    }
  }
  return nextState
}
