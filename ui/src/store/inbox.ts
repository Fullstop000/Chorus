import { queryOptions } from '@tanstack/react-query'
import type { ChannelInfo } from '../components/channels/types'
import { getInboxState } from '../data/inbox'
import type { InboxConversationState, ThreadInboxEntry } from '../data/inbox'

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

// ── Query definitions ──

export const inboxQueryKeys = {
  inbox: (user: string) => ['inbox', user] as const,
} as const

export const inboxQuery = (
  currentUser: string,
  shellBootstrapped: boolean,
  channelsData?: ChannelInfo[]
) =>
  queryOptions({
    queryKey: [
      ...inboxQueryKeys.inbox(currentUser),
      'bootstrapped',
      channelsData !== undefined,
    ],
    queryFn: async () => {
      const response = await getInboxState(currentUser)
      return { response, channels: channelsData ?? [] }
    },
    enabled: !!currentUser && !shellBootstrapped && channelsData !== undefined,
    staleTime: Infinity,
    select: ({ response, channels }) =>
      bootstrapInboxState(response.conversations, channels),
  })
