import { useCallback, useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { historyQueryKeys } from '../data'
import {
  normalizeEvent,
  bumpReplyCount,
  maxHistorySeq,
} from '../data/messages'
import { getSession } from '../transport'
import type { RealtimeFrame } from '../transport'
import type { HistoryMessage, HistoryResponse } from '../data'
import { useStore } from '../store'

interface UseHistoryOptions {
  threadParentId?: string | null
}

export function useHistory(
  username: string,
  targetKey: string | null,
  conversationId: string | null,
  options?: UseHistoryOptions
) {
  const queryClient = useQueryClient()
  const queryKey = historyQueryKeys.history(conversationId ?? '', options?.threadParentId ?? null)

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
        ? import('../data').then((m) => m.getHistory(conversationId, 50, options?.threadParentId ?? undefined))
        : Promise.resolve(null),
    enabled: !!username && !!targetKey && !!conversationId,
  })

  const messages = response?.messages ?? []
  const lastReadSeq = response?.last_read_seq ?? 0
  const loadedTarget = targetKey && !isLoading && !isError ? targetKey : null
  const { advanceConversationLatestSeq } = useStore()

  useEffect(() => {
    if (response && conversationId) {
      advanceConversationLatestSeq(conversationId, maxHistorySeq(response.messages))
    }
  }, [response, conversationId, advanceConversationLatestSeq])

  const commitMessages = useCallback(
    (updater: (current: HistoryMessage[]) => HistoryMessage[]) => {
      queryClient.setQueryData(queryKey, (current: HistoryResponse | undefined) => {
        if (!current) return current
        const next = updater(current.messages).sort((left, right) => left.seq - right.seq)
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
      if (frame.type === 'error') {
        console.error(frame.message)
        return
      }

      const msg = normalizeEvent(frame.event)
      if (!msg) return

      // Thread replies bump the parent's reply count but don't insert into root history
      if (msg.thread_parent_id) {
        commitMessages((current) => bumpReplyCount(current, msg.thread_parent_id!))
        return
      }

      // Seq-gated append: only insert if this message is newer than what we've seen
      const currentLatestSeq = useStore.getState().inboxState.conversations[conversationId]?.latestSeq ?? 0
      if (msg.seq <= currentLatestSeq) return

      advanceConversationLatestSeq(conversationId, msg.seq)
      queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
        if (!current) return current
        return { ...current, messages: [...current.messages, msg].sort((a, b) => a.seq - b.seq) }
      })
    }

    unsubscribeRealtime = getSession(username).subscribe(conversationId, handleFrame)

    return () => {
      cancelled = true
      unsubscribeRealtime?.()
    }
  }, [conversationId, options?.threadParentId, targetKey, username, queryClient, queryKey, commitMessages, advanceConversationLatestSeq])

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
