import { useCallback, useEffect, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { historyQueryKeys } from '../data'
import { getHistoryAfter } from '../data/chat'
import {
  normalizeEvent,
  maxHistorySeq,
} from '../data/messages'
import { getSession } from '../transport'
import type { RealtimeFrame } from '../transport'
import type { HistoryMessage, HistoryResponse } from '../data'
import { useStore } from '../store'

export function useHistory(
  username: string,
  targetKey: string | null,
  conversationId: string | null,
) {
  const queryClient = useQueryClient()
  const queryKey = historyQueryKeys.history(conversationId ?? '')

  const {
    data: response,
    isLoading,
    isError,
    error: queryError,
    refetch,
  } = useQuery({
    queryKey,
    queryFn: () =>
      conversationId
        ? import('../data').then((m) => m.getHistory(conversationId, 50))
        : Promise.resolve(null),
    enabled: !!username && !!targetKey && !!conversationId,
  })

  const messages = response?.messages ?? []
  const lastReadSeq = response?.last_read_seq ?? 0
  const loadedTarget = targetKey && !isLoading && !isError ? targetKey : null
  const { advanceConversationLatestSeq } = useStore()
  const storeLatestSeq = useStore((s) =>
    conversationId ? (s.inboxState.conversations[conversationId]?.latestSeq ?? 0) : 0
  )

  useEffect(() => {
    if (response && conversationId) {
      advanceConversationLatestSeq(conversationId, maxHistorySeq(response.messages))
    }
  }, [response, conversationId, advanceConversationLatestSeq])

  // Fetch gap messages when the global listener has advanced latestSeq beyond cached messages.
  // Instead of refetching the full history, fetch only messages after the cache's max seq.
  const isFetchingGapRef = useRef(false)
  useEffect(() => {
    if (!targetKey || !response || isLoading || !conversationId) return
    const cacheMaxSeq = maxHistorySeq(response.messages)
    if (storeLatestSeq <= cacheMaxSeq) return
    if (isFetchingGapRef.current) return
    isFetchingGapRef.current = true

    getHistoryAfter(conversationId, cacheMaxSeq, storeLatestSeq - cacheMaxSeq)
      .then((gap) => {
        if (gap.messages.length === 0) return
        queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
          if (!current) return current
          const existingMaxSeq = maxHistorySeq(current.messages)
          const newMessages = gap.messages.filter((m) => m.seq > existingMaxSeq)
          if (newMessages.length === 0) return current
          return { ...current, messages: [...current.messages, ...newMessages] }
        })
      })
      .finally(() => {
        isFetchingGapRef.current = false
      })
  }, [targetKey, storeLatestSeq, response, isLoading, conversationId, queryClient, queryKey])

  const commitMessages = useCallback(
    (updater: (current: HistoryMessage[]) => HistoryMessage[]) => {
      queryClient.setQueryData(queryKey, (current: HistoryResponse | undefined) => {
        if (!current) return current
        const next = updater(current.messages)
        if (conversationId) advanceConversationLatestSeq(conversationId, maxHistorySeq(next))
        return { ...current, messages: next }
      })
    },
    [queryClient, queryKey, conversationId, advanceConversationLatestSeq]
  )

  useEffect(() => {
    if (!username || !targetKey || !conversationId) return

    let cancelled = false
    let unsubscribeRealtime: (() => void) | null = null

    const handleFrame = (frame: RealtimeFrame) => {
      if (cancelled) return
      if (frame.type !== 'event') return

      const msg = normalizeEvent(frame.event)
      if (!msg) return

      // Append if newer than the last message in cache
      advanceConversationLatestSeq(conversationId, msg.seq)
      queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
        if (!current) return current
        const lastSeq = current.messages.length > 0 ? current.messages[current.messages.length - 1].seq : 0
        if (msg.seq <= lastSeq) return current
        return { ...current, messages: [...current.messages, msg] }
      })
    }

    unsubscribeRealtime = getSession(username).subscribe(conversationId, handleFrame)

    return () => {
      cancelled = true
      unsubscribeRealtime?.()
    }
  }, [conversationId, targetKey, username, queryClient, queryKey, commitMessages, advanceConversationLatestSeq])

  const appendMessage = useCallback(
    (message: HistoryMessage) => {
      commitMessages((current) => [...current, message])
    },
    [commitMessages]
  )

  return {
    messages,
    loading: isLoading,
    error: queryError ? String(queryError) : null,
    lastReadSeq,
    loadedTarget,
    refresh: refetch,
    appendMessage,
  }
}
