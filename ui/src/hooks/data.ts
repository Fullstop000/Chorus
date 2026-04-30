import { useCallback } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useStore } from '../store/uiStore'
import {
  agentsQuery,
  channelsQuery,
  channelMembersQuery,
  teamsQuery,
  humansQuery,
  channelQueryKeys,
  agentQueryKeys,
  teamQueryKeys,
  workspacesQuery,
  workspaceQueryKeys,
} from '../data'
import type { ChannelInfo } from '../data'
import type { InboxState } from '../store/inbox'
import { dmConversationNameForParticipants } from '../data'
import type { AgentInfo } from '../data'

function findAgentByName(agents: AgentInfo[], agentName: string): AgentInfo | undefined {
  return agents.find((agent) => agent.name === agentName)
}

function findDmChannelForAgentName(params: {
  currentHumanDisplayName: string
  currentHumanId: string
  agentName: string
  agents: AgentInfo[]
  dmChannels: ChannelInfo[]
}): ChannelInfo | undefined {
  const { currentHumanDisplayName, currentHumanId, agentName, agents, dmChannels } = params
  const agent = findAgentByName(agents, agentName)
  const candidates = new Set<string>()

  if (currentHumanId && agent?.id) {
    candidates.add(dmConversationNameForParticipants(currentHumanId, agent.id))
  }

  if (currentHumanDisplayName && agent?.name) {
    candidates.add(dmConversationNameForParticipants(currentHumanDisplayName, agent.name))
  }

  return dmChannels.find((channel) => candidates.has(channel.name))
}

function useAppInboxSelectors(params: {
  /** Local human display name — used only for DM channel title composition. */
  currentHumanDisplayName: string
  /** Local human id — canonical DM participant identity. */
  currentHumanId: string
  inboxState: InboxState
  agents: AgentInfo[]
  dmChannels: ChannelInfo[]
}) {
  const { currentHumanDisplayName, currentHumanId, inboxState, agents, dmChannels } = params

  const getConversationUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      const conv = inboxState.conversations[conversationId]
      if (!conv) return 0
      return Math.max(conv.latestSeq - conv.lastReadSeq, 0)
    },
    [inboxState.conversations]
  )

  const getAgentUnread = useCallback(
    (agentName: string) => {
      const conversationId = findDmChannelForAgentName({
        currentHumanDisplayName,
        currentHumanId,
        agentName,
        agents,
        dmChannels,
      })?.id ?? null
      return getConversationUnread(conversationId)
    },
    [currentHumanDisplayName, currentHumanId, agents, dmChannels, getConversationUnread]
  )

  const getAgentConversationId = useCallback(
    (agentName: string) => {
      return findDmChannelForAgentName({
        currentHumanDisplayName,
        currentHumanId,
        agentName,
        agents,
        dmChannels,
      })?.id ?? null
    },
    [currentHumanDisplayName, currentHumanId, agents, dmChannels]
  )

  return {
    getConversationUnread,
    getAgentUnread,
    getAgentConversationId,
  }
}

/** Registered agents. Empty array until auth + query settle. */
export function useAgents(): AgentInfo[] {
  const currentUserId = useStore((s) => s.currentUserId)
  const { data } = useQuery(agentsQuery(currentUserId))
  return data ?? []
}

/** Teams the current user belongs to. */
export function useTeams() {
  const currentUserId = useStore((s) => s.currentUserId)
  const { data } = useQuery(teamsQuery(currentUserId))
  return data ?? []
}

/** Human (non-agent) users on the server. */
export function useHumans() {
  const currentUserId = useStore((s) => s.currentUserId)
  const { data } = useQuery(humansQuery(currentUserId))
  return data ?? []
}

/** Platform workspaces for the running Chorus server. */
export function useWorkspaces() {
  const { data, isLoading, error } = useQuery(workspacesQuery)
  return {
    workspaces: data ?? [],
    isLoading,
    error,
  }
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
  const currentUserId = useStore((s) => s.currentUserId)
  const { data } = useQuery(channelsQuery(currentUserId))
  return data ?? { allChannels: [], channels: [], systemChannels: [], dmChannels: [] }
}

/**
 * Cache-invalidation actions: refreshChannels, refreshAgents, refreshTeams, refreshServerInfo.
 */
export function useRefresh() {
  const currentUserId = useStore((s) => s.currentUserId)
  const queryClient = useQueryClient()

  const refreshChannels = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: channelQueryKeys.channels(currentUserId) })
  }, [currentUserId, queryClient])

  const refreshAgents = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: agentQueryKeys.agents(currentUserId) })
  }, [currentUserId, queryClient])

  const refreshTeams = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: teamQueryKeys.teams })
  }, [queryClient])

  const refreshServerInfo = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: agentQueryKeys.agents(currentUserId) }),
      queryClient.invalidateQueries({ queryKey: channelQueryKeys.channels(currentUserId) }),
      queryClient.invalidateQueries({ queryKey: teamQueryKeys.teams }),
      queryClient.invalidateQueries({ queryKey: channelQueryKeys.humans(currentUserId) }),
      queryClient.invalidateQueries({ queryKey: workspaceQueryKeys.workspaces }),
      queryClient.invalidateQueries({ queryKey: workspaceQueryKeys.current }),
    ])
  }, [currentUserId, queryClient])

  return {
    refreshChannels,
    refreshAgents,
    refreshTeams,
    refreshServerInfo,
  }
}

/**
 * Inbox selectors.
 * Returns unread counts per conversation/agent.
 */
export function useInbox() {
  const currentUser = useStore((s) => s.currentUser)
  const currentUserId = useStore((s) => s.currentUserId)
  const inboxState = useStore((s) => s.inboxState)
  const agents = useAgents()
  const { dmChannels } = useChannels()

  const selectors = useAppInboxSelectors({
    currentHumanDisplayName: currentUser,
    currentHumanId: currentUserId,
    inboxState,
    agents,
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
