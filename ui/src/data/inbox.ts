import { get } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import { bootstrapInboxState } from '../inbox'
import type {
  ChannelInfo,
  InboxResponse,
  ConversationInboxRefreshResponse,
  ThreadInboxResponse,
} from '../types'

export type {
  InboxConversationState,
  InboxResponse,
  ConversationInboxRefreshResponse,
  ThreadInboxEntry,
  ThreadInboxResponse,
} from '../types'

function conversationPath(conversationId: string, suffix = ''): string {
  return `/api/conversations/${encodeURIComponent(conversationId)}${suffix}`
}

export function getInboxState(_username: string): Promise<InboxResponse> {
  return get('/api/inbox')
}

export function getConversationInboxNotification(
  conversationId: string,
  threadParentId?: string
): Promise<ConversationInboxRefreshResponse> {
  return get(
    `${conversationPath(conversationId, '/inbox-notification')}${queryString({ threadParentId })}`
  )
}

export function getChannelThreads(conversationId: string): Promise<ThreadInboxResponse> {
  return get(conversationPath(conversationId, '/threads'))
}

export function sortByUnread<T extends { unreadCount: number }>(items: T[]): T[] {
  return [...items].sort((a, b) => b.unreadCount - a.unreadCount)
}

export function hasUnread(item: { unreadCount: number }): boolean {
  return item.unreadCount > 0
}

export const inboxQueryKeys = {
  inbox: (user: string) => ['inbox', user] as const,
} as const

export type InboxState = ReturnType<typeof bootstrapInboxState>

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
