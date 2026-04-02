import { useEffect } from 'react'
import type { QueryClient } from '@tanstack/react-query'
import { ensureDirectMessageConversation } from '../data'
import type { ChannelInfo } from '../data'
import {
  dmConversationNameForParticipants,
  ensureInboxConversations,
  type InboxState,
} from '../inbox'
import { isVisibleSidebarChannel } from '../pages/Sidebar/sidebarChannels'
import { useStore } from './uiStore'
import { channelQueryKeys } from '../data'

export function syncWhoami(
  username: string | undefined,
  currentUser: string,
  setCurrentUser: (u: string) => void,
  resetUserSession: () => void
): void {
  useEffect(() => {
    if (!username) return
    if (username === currentUser) return
    if (currentUser) resetUserSession()
    setCurrentUser(username)
  }, [username, currentUser, setCurrentUser, resetUserSession])
}

export function mirrorChannels(
  allChannels: ChannelInfo[],
  updateInboxState: (u: (c: InboxState) => InboxState) => void
): void {
  useEffect(() => {
    if (!allChannels.length) return
    updateInboxState((current) => ensureInboxConversations(current, allChannels))
  }, [allChannels, updateInboxState])
}

export function autoSelectChannel(params: {
  shellBootstrapped: boolean
  channels: ChannelInfo[]
  systemChannels: ChannelInfo[]
  setCurrentChannel: (channel: ChannelInfo | null) => void
}): void {
  const { shellBootstrapped, channels, systemChannels, setCurrentChannel } = params

  useEffect(() => {
    if (!shellBootstrapped) return
    const { currentAgent, currentChannel } = useStore.getState()
    if (currentAgent) return

    const joinedChannels = [
      ...systemChannels.filter((c) => c.joined),
      ...channels.filter(isVisibleSidebarChannel),
    ]

    if (currentChannel && joinedChannels.some((c) => c.id === currentChannel.id || c.name === currentChannel.name)) return

    setCurrentChannel(joinedChannels[0] ?? null)
  }, [shellBootstrapped, channels, systemChannels, setCurrentChannel])
}

export function ensureAgentDm(params: {
  currentUser: string
  currentAgentName: string | null
  dmChannels: ChannelInfo[]
  queryClient: QueryClient
  updateInboxState: (u: (c: InboxState) => InboxState) => void
}): void {
  const { currentUser, currentAgentName, dmChannels, queryClient, updateInboxState } = params

  useEffect(() => {
    if (!currentUser || !currentAgentName) return
    const dmName = dmConversationNameForParticipants(currentUser, currentAgentName)
    if (dmChannels.some((ch: ChannelInfo) => ch.name === dmName)) return

    let cancelled = false
    ensureDirectMessageConversation(currentAgentName)
      .then((channel) => {
        if (cancelled) return
        queryClient.setQueryData<ChannelInfo[]>(channelQueryKeys.channels(currentUser), (current = []) => {
          if (current.some((ch: ChannelInfo) => ch.id === channel.id || ch.name === channel.name)) {
            return current
          }
          return [...current, channel]
        })
        updateInboxState((current: InboxState) => ensureInboxConversations(current, [channel]))
      })
      .catch((error) => {
        if (!cancelled) console.error('Failed to ensure DM conversation', error)
      })

    return () => {
      cancelled = true
    }
  }, [currentUser, dmChannels, currentAgentName, queryClient, updateInboxState])
}
