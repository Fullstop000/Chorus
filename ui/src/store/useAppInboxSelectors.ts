import type { ChannelInfo, ThreadInboxEntry } from '../data'
import { useCallback } from 'react'
import {
  conversationThreadUnreadCount,
  dmConversationNameForParticipants,
  mergeChannelThreadInboxEntries,
  type InboxState,
} from '../inbox'

export function useAppInboxSelectors(params: {
  currentUser: string
  inboxState: InboxState
  conversationThreads: Record<string, ThreadInboxEntry[]>
  dmChannels: ChannelInfo[]
}) {
  const { currentUser, inboxState, conversationThreads, dmChannels } = params

  const getConversationUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      return inboxState.conversations[conversationId]?.unreadCount ?? 0
    },
    [inboxState.conversations]
  )

  const getConversationThreadUnreadCount = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      return inboxState.conversations[conversationId]?.threadUnreadCount ?? 0
    },
    [inboxState.conversations]
  )

  const getConversationThreads = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return []
      return mergeChannelThreadInboxEntries(
        conversationThreads[conversationId] ?? [],
        inboxState,
        conversationId
      )
    },
    [conversationThreads, inboxState]
  )

  const getConversationThreadUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      const cached = conversationThreads[conversationId]
      if (cached && cached.length > 0) {
        return mergeChannelThreadInboxEntries(cached, inboxState, conversationId).reduce(
          (sum, entry) => sum + entry.unreadCount,
          0
        )
      }
      return conversationThreadUnreadCount(inboxState, conversationId)
    },
    [conversationThreads, inboxState]
  )

  const getAgentUnread = useCallback(
    (agentName: string) => {
      const dmName = dmConversationNameForParticipants(currentUser, agentName)
      const conversationId = dmChannels.find((ch: ChannelInfo) => ch.name === dmName)?.id ?? null
      return getConversationUnread(conversationId)
    },
    [currentUser, dmChannels, getConversationUnread]
  )

  const getAgentConversationId = useCallback(
    (agentName: string) => {
      const dmName = dmConversationNameForParticipants(currentUser, agentName)
      return dmChannels.find((ch: ChannelInfo) => ch.name === dmName)?.id ?? null
    },
    [currentUser, dmChannels]
  )

  return {
    getConversationUnread,
    getConversationThreadUnreadCount,
    getConversationThreads,
    getConversationThreadUnread,
    getAgentUnread,
    getAgentConversationId,
  }
}
