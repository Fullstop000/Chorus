import { get } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import { bootstrapInboxState } from '../inbox'

// ── Types (source of truth — API responses) ──

export interface InboxConversationState {
  conversationId: string
  conversationName: string
  conversationType: string
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  threadUnreadCount: number
  lastReadMessageId?: string | null
  lastMessageId?: string | null
  lastMessageAt?: string | null
}

export interface InboxResponse {
  conversations: InboxConversationState[]
}

export interface ConversationInboxRefreshResponse {
  conversation: InboxConversationState
  thread?: {
    conversationId: string
    threadParentId: string
    latestSeq: number
    lastReadSeq: number
    unreadCount: number
    lastReplyMessageId?: string | null
    lastReplyAt?: string | null
  }
}

export interface ThreadInboxEntry {
  conversationId: string
  threadParentId: string
  parentSeq: number
  parentSenderName: string
  parentSenderType: 'human' | 'agent'
  parentContent: string
  parentCreatedAt: string
  replyCount: number
  participantCount: number
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  lastReplyMessageId?: string | null
  lastReplyAt?: string | null
}

export interface ThreadInboxResponse {
  unreadCount: number
  threads: ThreadInboxEntry[]
}

// ── API functions ──

export function getInboxState(_username: string): Promise<InboxResponse> {
  return get('/api/inbox')
}

export function getConversationInboxNotification(
  conversationId: string,
  threadParentId?: string
): Promise<ConversationInboxRefreshResponse> {
  return get(
    `/api/conversations/${encodeURIComponent(conversationId)}/inbox-notification${queryString({ threadParentId })}`
  )
}

export function getChannelThreads(conversationId: string): Promise<ThreadInboxResponse> {
  return get(`/api/conversations/${encodeURIComponent(conversationId)}/threads`)
}

// ── Transforms ──

export function sortByUnread<T extends { unreadCount: number }>(items: T[]): T[] {
  return [...items].sort((a, b) => b.unreadCount - a.unreadCount)
}

export function hasUnread(item: { unreadCount: number }): boolean {
  return item.unreadCount > 0
}

// ── Query definitions ──

export const inboxQueryKeys = {
  inbox: (user: string) => ['inbox', user] as const,
} as const

export type InboxState = ReturnType<typeof bootstrapInboxState>

export const inboxQuery = (
  currentUser: string,
  shellBootstrapped: boolean,
  channelsData?: import('./channels').ChannelInfo[]
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
