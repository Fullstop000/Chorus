import { useCallback, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useStore } from '../store/uiStore'
import {
  agentsQuery,
  channelsQuery,
  channelMembersQuery,
  teamsQuery,
  humansQuery,
  getChannelThreads,
  channelQueryKeys,
  agentQueryKeys,
  teamQueryKeys,
} from '../data'
import type { ChannelInfo, ThreadInboxEntry } from '../data'
import {
  conversationThreadUnreadCount,
  dmConversationNameForParticipants,
  mergeChannelThreadInboxEntries,
  type InboxState,
} from '../inbox'
import type { AgentInfo } from '../data'

function useAppInboxSelectors(params: {
  currentUser: string
  inboxState: InboxState
  conversationThreads: Record<string, ThreadInboxEntry[]>
  dmChannels: ChannelInfo[]
}) {
  const { currentUser, inboxState, conversationThreads, dmChannels } = params

  const getConversationUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      const conv = inboxState.conversations[conversationId]
      if (!conv) return 0
      return Math.max(conv.latestSeq - conv.lastReadSeq, 0)
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

/** Registered agents. Empty array until auth + query settle. */
export function useAgents(): AgentInfo[] {
  const currentUser = useStore((s) => s.currentUser)
  const { data } = useQuery(agentsQuery(currentUser))
  return data ?? []
}

/** Teams the current user belongs to. */
export function useTeams() {
  const currentUser = useStore((s) => s.currentUser)
  const { data } = useQuery(teamsQuery(currentUser))
  return data ?? []
}

/** Human (non-agent) users on the server. */
export function useHumans() {
  const currentUser = useStore((s) => s.currentUser)
  const { data } = useQuery(humansQuery(currentUser))
  return data ?? []
}

/** Members of a specific channel. Returns empty array for DMs or when no channelId. */
export function useChannelMembers(channelId: string | null) {
  const { data } = useQuery(channelMembersQuery(channelId))
  return data?.members ?? []
}

/**
 * All channels the user can see, pre-sliced into regular / system / DM.
 * Returns `{ allChannels, channels, systemChannels, dmChannels }`.
 */
export function useChannels() {
  const currentUser = useStore((s) => s.currentUser)
  const { data } = useQuery(channelsQuery(currentUser))
  return data ?? { allChannels: [], channels: [], systemChannels: [], dmChannels: [] }
}

/**
 * Cache-invalidation actions: refreshConversationThreads (with in-flight
 * dedup), refreshChannels, refreshAgents, refreshTeams, refreshServerInfo.
 */
export function useRefresh() {
  const currentUser = useStore((s) => s.currentUser)
  const queryClient = useQueryClient()
  const setConversationThreads = useStore((s) => s.setConversationThreads)
  const conversationThreadsInFlight = useRef<Map<string, Promise<void>>>(new Map())

  const refreshConversationThreads = useCallback(
    async (conversationId: string) => {
      if (!currentUser) return
      const inFlight = conversationThreadsInFlight.current
      const existing = inFlight.get(conversationId)
      if (existing) return existing
      const promise = (async () => {
        try {
          const response = await getChannelThreads(conversationId)
          setConversationThreads(conversationId, response.threads)
        } catch (error) {
          console.error('Failed to load channel threads', error)
        } finally {
          inFlight.delete(conversationId)
        }
      })()
      inFlight.set(conversationId, promise)
      return promise
    },
    [currentUser, setConversationThreads]
  )

  const refreshChannels = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: channelQueryKeys.channels(currentUser) })
  }, [currentUser, queryClient])

  const refreshAgents = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: agentQueryKeys.agents })
  }, [queryClient])

  const refreshTeams = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: teamQueryKeys.teams })
  }, [queryClient])

  const refreshServerInfo = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: agentQueryKeys.agents }),
      queryClient.invalidateQueries({ queryKey: channelQueryKeys.channels(currentUser) }),
      queryClient.invalidateQueries({ queryKey: teamQueryKeys.teams }),
      queryClient.invalidateQueries({ queryKey: channelQueryKeys.humans }),
    ])
  }, [currentUser, queryClient])

  return {
    refreshConversationThreads,
    refreshChannels,
    refreshAgents,
    refreshTeams,
    refreshServerInfo,
  }
}

/**
 * Inbox selectors.
 * Returns unread counts per conversation/thread/agent and thread listings.
 */
export function useInbox() {
  const currentUser = useStore((s) => s.currentUser)
  const inboxState = useStore((s) => s.inboxState)
  const conversationThreads = useStore((s) => s.conversationThreads)
  const { dmChannels } = useChannels()

  const selectors = useAppInboxSelectors({
    currentUser,
    inboxState,
    conversationThreads,
    dmChannels,
  })

  return selectors
}

/** Backend routing key for the current selection. Centralizes the `#`/`dm:@` prefix. */
export function useTarget(): string | null {
  const currentChannel = useStore((s) => s.currentChannel)
  const currentAgent = useStore((s) => s.currentAgent)
  if (currentChannel) return `#${currentChannel.name}`
  if (currentAgent) return `dm:@${currentAgent.name}`
  return null
}
