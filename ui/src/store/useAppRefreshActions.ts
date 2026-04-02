import { useCallback, useRef } from 'react'
import type { QueryClient } from '@tanstack/react-query'
import { getChannelThreads, channelQueryKeys, agentQueryKeys, teamQueryKeys } from '../data'
import type { ThreadInboxEntry } from '../data'

export function useAppRefreshActions(params: {
  currentUser: string
  queryClient: QueryClient
  setConversationThreads: (conversationId: string, threads: ThreadInboxEntry[]) => void
}) {
  const { currentUser, queryClient, setConversationThreads } = params
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
