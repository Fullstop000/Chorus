import { useCallback, useEffect, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { updateReadCursor, historyQueryKeys } from '../data'
import {
  normalizeEvent,
  bumpReplyCount,
  maxHistorySeq,
} from '../data/messages'
import { getSession } from '../transport'
import type { RealtimeFrame } from '../transport'
import type { HistoryMessage, HistoryResponse } from '../data'
import type { ReadCursorAckPayload } from '../inbox'
import { useStore } from '../store'

interface UseHistoryOptions {
  threadParentId?: string | null
  onReadCursorAck?: (ack: ReadCursorAckPayload) => void
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
  const lastReadSeqRef = useRef(0)
  const pendingReadSeqRef = useRef<number | null>(null)
  const readCursorTimerRef = useRef<number | null>(null)
  const { addUnreadMessageId, advanceConversationLatestSeq, unreadMessageIds } = useStore()

  useEffect(() => {
    if (response && conversationId) {
      advanceConversationLatestSeq(conversationId, maxHistorySeq(response.messages))
    }
  }, [response, conversationId, advanceConversationLatestSeq])

  useEffect(() => {
    lastReadSeqRef.current = lastReadSeq
  }, [lastReadSeq])

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
      if (msg.senderName !== username) {
        addUnreadMessageId(targetKey!, msg.id)
      }
      queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
        if (!current) return current
        return { ...current, messages: [...current.messages, msg].sort((a, b) => a.seq - b.seq) }
      })
    }

    unsubscribeRealtime = getSession(username).subscribe(conversationId, handleFrame)

    return () => {
      cancelled = true
      if (readCursorTimerRef.current != null) {
        window.clearTimeout(readCursorTimerRef.current)
        readCursorTimerRef.current = null
      }
      unsubscribeRealtime?.()
    }
  }, [conversationId, options?.threadParentId, targetKey, username, queryClient, queryKey, commitMessages, addUnreadMessageId, advanceConversationLatestSeq])

  const reportVisibleSeq = useCallback(
    (visibleSeq: number) => {
      if (!username || !targetKey || !conversationId || visibleSeq <= 0) return
      if (loadedTarget !== targetKey) return
      if (document.visibilityState !== 'visible') return
      const nextSeq = Math.max(visibleSeq, pendingReadSeqRef.current ?? 0)
      if (nextSeq <= lastReadSeqRef.current) return
      pendingReadSeqRef.current = nextSeq
      if (readCursorTimerRef.current != null) return

      readCursorTimerRef.current = window.setTimeout(async () => {
        readCursorTimerRef.current = null
        const flushSeq = pendingReadSeqRef.current
        pendingReadSeqRef.current = null
        if (flushSeq == null || flushSeq <= lastReadSeqRef.current) return
        if (document.visibilityState !== 'visible') return
        try {
          const res = await updateReadCursor(
            conversationId,
            flushSeq,
            options?.threadParentId || undefined
          )
          queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) =>
            current ? { ...current, last_read_seq: Math.max(current.last_read_seq ?? 0, flushSeq) } : current
          )
          options?.onReadCursorAck?.({
            conversationId,
            conversationUnreadCount: res.conversationUnreadCount,
            conversationLastReadSeq: res.conversationLastReadSeq,
            conversationLatestSeq: res.conversationLatestSeq,
            conversationThreadUnreadCount: res.conversationThreadUnreadCount,
            threadParentId: res.threadParentId ?? null,
            threadUnreadCount: res.threadUnreadCount,
            threadLastReadSeq: res.threadLastReadSeq,
            threadLatestSeq: res.threadLatestSeq,
          })
        } catch (cursorError) {
          console.error('Failed to update read cursor', cursorError)
        }
      }, 150)
    },
    [conversationId, loadedTarget, options?.onReadCursorAck, options?.threadParentId, targetKey, username, queryClient, queryKey]
  )

  const appendMessage = useCallback(
    (message: HistoryMessage) => {
      commitMessages((current) => [...current, message])
    },
    [commitMessages]
  )

  const unreadIds: Set<string> = targetKey ? (unreadMessageIds[targetKey] ?? new Set()) : new Set()

  return {
    messages,
    loading: isLoading,
    error: queryError ? String(queryError) : null,
    lastReadSeq,
    loadedTarget,
    refresh: refetch,
    reportVisibleSeq,
    unreadIds,
    appendMessage,
  }
}
