import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useStore } from '../store/uiStore'
import {
  agentsQuery,
  channelsQuery,
  teamsQuery,
  humansQuery,
} from '../data'
import { useAppInboxSelectors } from '../store/useAppInboxSelectors'
import { useAppRefreshActions } from '../store/useAppRefreshActions'
import { applyReadCursorAck } from '../App'
import type { AgentInfo } from '../data'

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
 * Cache-invalidation actions: refreshConversationThreads, refreshChannels,
 * refreshAgents, refreshTeams, refreshServerInfo.
 */
export function useRefresh() {
  const currentUser = useStore((s) => s.currentUser)
  const queryClient = useQueryClient()
  const setConversationThreads = useStore((s) => s.setConversationThreads)
  return useAppRefreshActions({ currentUser, queryClient, setConversationThreads })
}

/**
 * Inbox selectors + read-cursor ack handler.
 * Returns unread counts per conversation/thread/agent, thread listings,
 * and `applyReadCursorAck` for marking threads as read.
 */
export function useInbox() {
  const currentUser = useStore((s) => s.currentUser)
  const inboxState = useStore((s) => s.inboxState)
  const conversationThreads = useStore((s) => s.conversationThreads)
  const { dmChannels } = useChannels()
  const queryClient = useQueryClient()

  const selectors = useAppInboxSelectors({
    currentUser,
    inboxState,
    conversationThreads,
    dmChannels,
  })

  const applyReadCursorAckFn = applyReadCursorAck({ queryClient })

  return { ...selectors, applyReadCursorAck: applyReadCursorAckFn }
}

/** Backend routing key for the current selection. Centralizes the `#`/`dm:@` prefix. */
export function useTarget(): string | null {
  const currentChannel = useStore((s) => s.currentChannel)
  const currentAgent = useStore((s) => s.currentAgent)
  if (currentChannel) return `#${currentChannel.name}`
  if (currentAgent) return `dm:@${currentAgent.name}`
  return null
}
