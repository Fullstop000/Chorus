import { useCallback, useEffect, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getHistoryAfter, updateReadCursor, historyQueryKeys } from '../data'
import {
  applyRealtimeEvent,
  historyFetchAfterForNotification,
  maxHistorySeq,
  mergeHistoryMessages,
} from '../transport/realtime'
import { getRealtimeSession } from '../transport/realtimeSession'
import type { HistoryMessage, HistoryResponse } from '../data'
import type { ReadCursorAckPayload } from '../inbox'

interface UseHistoryOptions {
  threadParentId?: string | null
  onReadCursorAck?: (ack: ReadCursorAckPayload) => void
}

interface OptimisticMessageHandle {
  tempId: string
  clientNonce: string
}

function createClientNonce(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `client:${Date.now()}:${Math.random().toString(16).slice(2)}`
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
  const maxLoadedSeqRef = useRef(0)
  const lastReadSeqRef = useRef(0)
  const pendingReadSeqRef = useRef<number | null>(null)
  const readCursorTimerRef = useRef<number | null>(null)

  useEffect(() => {
    if (response) maxLoadedSeqRef.current = maxHistorySeq(response.messages)
  }, [response])

  useEffect(() => {
    lastReadSeqRef.current = lastReadSeq
  }, [lastReadSeq])

  const commitMessages = useCallback(
    (updater: (current: HistoryMessage[]) => HistoryMessage[]) => {
      queryClient.setQueryData(queryKey, (current: HistoryResponse | undefined) => {
        if (!current) return current
        const next = updater(current.messages).sort((left, right) => left.seq - right.seq)
        maxLoadedSeqRef.current = maxHistorySeq(next)
        return { ...current, messages: next }
      })
    },
    [queryClient, queryKey]
  )

  useEffect(() => {
    if (!username || !targetKey || !conversationId) return

    let cancelled = false
    let unsubscribeRealtime: (() => void) | null = null
    let activeRealtimeTarget: string | null = null

    async function bootstrap() {
      try {
        activeRealtimeTarget = `conversation:${conversationId}`
      } catch (targetError) {
        if (!cancelled) {
          console.error(targetError instanceof Error ? targetError.message : String(targetError))
        }
        return
      }

      unsubscribeRealtime = getRealtimeSession(username).subscribe({
        targets: [activeRealtimeTarget],
        onFrame: (frame) => {
          if (cancelled) return
          if (frame.type === 'error') {
            console.error(frame.message)
            return
          }

          const didOptimisticAppend = queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
            if (!current) return current
            const before = current.messages.length
            const updated = applyRealtimeEvent(current.messages, frame.event)
            if (updated.length > before) {
              maxLoadedSeqRef.current = maxHistorySeq(updated)
              return { ...current, messages: updated }
            }
            return current
          })

          if (didOptimisticAppend) return

          const incrementalAfter = historyFetchAfterForNotification(
            activeRealtimeTarget,
            frame.event,
            maxLoadedSeqRef.current,
            options?.threadParentId ?? null
          )
          if (incrementalAfter != null && conversationId) {
            void getHistoryAfter(conversationId, incrementalAfter, 50, options?.threadParentId ?? undefined).then(
              (res) => {
                if (cancelled) return
                queryClient.setQueryData<HistoryResponse | undefined>(queryKey, (current) => {
                  if (!current) return current
                  const merged = mergeHistoryMessages(current.messages, res.messages)
                  maxLoadedSeqRef.current = maxHistorySeq(merged)
                  return { ...current, messages: merged, last_read_seq: res.last_read_seq ?? current.last_read_seq }
                })
              }
            )
          } else if (frame.event.eventType === 'message.created' && conversationId) {
            void refetch()
          }
        },
      })
    }

    void bootstrap()

    return () => {
      cancelled = true
      if (readCursorTimerRef.current != null) {
        window.clearTimeout(readCursorTimerRef.current)
        readCursorTimerRef.current = null
      }
      unsubscribeRealtime?.()
    }
  }, [conversationId, options?.threadParentId, targetKey, username, queryClient, queryKey, refetch])

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

  const addOptimisticMessage = useCallback(
    (draft: { content: string; attachments?: HistoryMessage['attachments'] }): OptimisticMessageHandle => {
      const tempId = `client:${Date.now()}:${Math.random().toString(16).slice(2)}`
      const clientNonce = createClientNonce()
      const optimisticMessage: HistoryMessage = {
        id: tempId,
        seq: maxLoadedSeqRef.current + 1,
        content: draft.content,
        senderName: username,
        senderType: 'human',
        senderDeleted: false,
        createdAt: new Date().toISOString(),
        attachments: draft.attachments,
        clientNonce,
        clientStatus: 'sending',
      }
      commitMessages((current) => [...current, optimisticMessage])
      return { tempId, clientNonce }
    },
    [commitMessages, username]
  )

  const ackOptimisticMessage = useCallback(
    (handle: OptimisticMessageHandle, ack: { messageId: string; seq: number; createdAt: string; clientNonce?: string }) => {
      const nonce = ack.clientNonce ?? handle.clientNonce
      commitMessages((current) =>
        current.map((message) =>
          message.clientNonce === nonce || message.id === handle.tempId
            ? {
                ...message,
                id: ack.messageId,
                seq: ack.seq,
                createdAt: ack.createdAt,
                clientNonce: nonce,
                clientStatus: undefined,
                clientError: undefined,
              }
            : message
        )
      )
    },
    [commitMessages]
  )

  const failOptimisticMessage = useCallback(
    (handle: OptimisticMessageHandle, errorMessage: string) => {
      commitMessages((current) =>
        current.map((message) =>
          message.clientNonce === handle.clientNonce || message.id === handle.tempId
            ? { ...message, clientStatus: 'failed', clientError: errorMessage }
            : message
        )
      )
    },
    [commitMessages]
  )

  const retryOptimisticMessage = useCallback(
    (messageId: string): OptimisticMessageHandle | null => {
      const nextHandle = { tempId: messageId, clientNonce: createClientNonce() }
      commitMessages((current) =>
        current.map((message) => {
          if (message.id !== messageId) return message
          return { ...message, clientNonce: nextHandle.clientNonce, clientStatus: 'sending', clientError: undefined }
        })
      )
      return nextHandle
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
    reportVisibleSeq,
    addOptimisticMessage,
    ackOptimisticMessage,
    failOptimisticMessage,
    retryOptimisticMessage,
  }
}
