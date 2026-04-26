import { queryOptions } from '@tanstack/react-query'
import type { ChannelInfo } from '../components/channels/types'
import { getInboxState } from '../data/inbox'
import type { InboxConversationState } from '../data/inbox'

export interface InboxState {
  conversations: Record<string, InboxConversationState>
}

export function createInboxState(): InboxState {
  return {
    conversations: {},
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

// ── Query definitions ──

export const inboxQueryKeys = {
  inbox: (humanId: string) => ['inbox', humanId] as const,
} as const

export const inboxQuery = (
  memberHumanId: string,
  shellBootstrapped: boolean,
  channelsData?: ChannelInfo[]
) =>
  queryOptions({
    queryKey: [
      ...inboxQueryKeys.inbox(memberHumanId),
      'bootstrapped',
      channelsData !== undefined,
    ],
    queryFn: async () => {
      const response = await getInboxState(memberHumanId)
      return { response, channels: channelsData ?? [] }
    },
    enabled: !!memberHumanId && !shellBootstrapped && channelsData !== undefined,
    staleTime: Infinity,
    select: ({ response, channels }) =>
      bootstrapInboxState(response.conversations, channels),
  })
