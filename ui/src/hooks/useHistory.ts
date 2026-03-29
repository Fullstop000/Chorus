import { useCallback, useEffect, useRef, useState } from 'react'
import { getHistory, getHistoryAfter, updateReadCursor } from '../api'
import {
  applyRealtimeEvent,
  historyFetchAfterForNotification,
  maxHistorySeq,
  mergeHistoryMessages,
} from '../transport/realtime'
import { getRealtimeSession } from '../transport/realtimeSession'
import type { HistoryMessage, HistoryResponse } from '../types'
import { loadSharedRequest } from './historyRequestCache'

interface UseHistoryOptions {
  threadParentId?: string | null
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
  const [messages, setMessages] = useState<HistoryMessage[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [lastReadSeq, setLastReadSeq] = useState(0)
  const [loadedTarget, setLoadedTarget] = useState<string | null>(null)
  const maxLoadedSeqRef = useRef(0)
  const incrementalFetchAfterRef = useRef<number | null>(null)
  const lastReadSeqRef = useRef(0)
  const pendingReadSeqRef = useRef<number | null>(null)
  const readCursorTimerRef = useRef<number | null>(null)

  useEffect(() => {
    lastReadSeqRef.current = lastReadSeq
  }, [lastReadSeq])

  const commitMessages = useCallback((updater: (current: HistoryMessage[]) => HistoryMessage[]) => {
    setMessages((current) => {
      const next = updater(current).sort((left, right) => left.seq - right.seq)
      maxLoadedSeqRef.current = maxHistorySeq(next)
      return next
    })
  }, [])

  const fetchHistory = useCallback(async (after?: number): Promise<HistoryResponse | null> => {
    if (!username || !targetKey || !conversationId) return null
    if (after != null && incrementalFetchAfterRef.current === after) {
      return null
    }
    if (after != null) {
      incrementalFetchAfterRef.current = after
    }
    try {
      const res =
        after != null
          ? await getHistoryAfter(
              conversationId,
              after,
              50,
              options?.threadParentId ?? undefined
            )
          : await loadSharedRequest(
              `history:${conversationId}:${options?.threadParentId ?? 'root'}:bootstrap`,
              () => getHistory(conversationId, 50, options?.threadParentId ?? undefined)
            )
      if (after != null) {
        commitMessages((current) => {
          const merged = mergeHistoryMessages(current, res.messages)
          return merged
        })
      } else {
        setMessages(res.messages)
        maxLoadedSeqRef.current = maxHistorySeq(res.messages)
      }
      setLastReadSeq(res.last_read_seq ?? 0)
      setLoadedTarget(targetKey)
      setError(null)
      return res
    } catch (e) {
      setError(String(e))
      return null
    } finally {
      if (after == null) {
        setLoading(false)
      }
      if (incrementalFetchAfterRef.current === after) {
        incrementalFetchAfterRef.current = null
      }
    }
  }, [conversationId, options?.threadParentId, targetKey, username])

  useEffect(() => {
    if (!username || !targetKey || !conversationId) {
      setMessages([])
      setError(null)
      setLastReadSeq(0)
      setLoadedTarget(null)
      maxLoadedSeqRef.current = 0
      incrementalFetchAfterRef.current = null
      return
    }

    let cancelled = false
    let unsubscribeRealtime: (() => void) | null = null
    let activeRealtimeTarget: string | null = null
    async function bootstrap() {
      setLoading(true)
      setMessages([])
      setError(null)
      setLastReadSeq(0)
      setLoadedTarget(null)
      maxLoadedSeqRef.current = 0
      incrementalFetchAfterRef.current = null
      pendingReadSeqRef.current = null
      if (readCursorTimerRef.current != null) {
        window.clearTimeout(readCursorTimerRef.current)
        readCursorTimerRef.current = null
      }

      const history = await fetchHistory()
      if (cancelled || !history) return

      try {
        activeRealtimeTarget = options?.threadParentId
          ? `thread:${options.threadParentId}`
          : `conversation:${conversationId}`
      } catch (targetError) {
        if (!cancelled) {
          setError(targetError instanceof Error ? targetError.message : String(targetError))
        }
        return
      }

      unsubscribeRealtime = getRealtimeSession(username).subscribe({
        targets: [activeRealtimeTarget],
        onFrame: (frame) => {
          if (cancelled) return
          if (frame.type === 'subscribed') {
            return
          }
          if (frame.type === 'error') {
            setError(frame.message)
            return
          }
          const incrementalAfter = historyFetchAfterForNotification(
            activeRealtimeTarget,
            frame.event,
            maxLoadedSeqRef.current
          )
          if (incrementalAfter != null) {
            void fetchHistory(incrementalAfter)
            return
          }
          commitMessages((current) => applyRealtimeEvent(current, frame.event))
          setError(null)
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
  }, [conversationId, fetchHistory, options?.threadParentId, targetKey, username])

  const reportVisibleSeq = useCallback((visibleSeq: number) => {
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
        await updateReadCursor(
          conversationId,
          flushSeq,
          options?.threadParentId ?? undefined
        )
        setLastReadSeq((current) => Math.max(current, flushSeq))
      } catch (cursorError) {
        console.error('Failed to update read cursor', cursorError)
      }
    }, 150)
  }, [conversationId, loadedTarget, options?.threadParentId, targetKey, username])

  const addOptimisticMessage = useCallback((draft: {
    content: string
    attachments?: HistoryMessage['attachments']
  }): OptimisticMessageHandle => {
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
  }, [commitMessages, username])

  const ackOptimisticMessage = useCallback((handle: OptimisticMessageHandle, ack: {
    messageId: string
    seq: number
    createdAt: string
    clientNonce?: string
  }) => {
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
  }, [commitMessages])

  const failOptimisticMessage = useCallback((handle: OptimisticMessageHandle, errorMessage: string) => {
    commitMessages((current) =>
      current.map((message) =>
        message.clientNonce === handle.clientNonce || message.id === handle.tempId
          ? {
              ...message,
              clientStatus: 'failed',
              clientError: errorMessage,
            }
          : message
      )
    )
  }, [commitMessages])

  const retryOptimisticMessage = useCallback((messageId: string): OptimisticMessageHandle | null => {
    const nextHandle = {
      tempId: messageId,
      clientNonce: createClientNonce(),
    }
    commitMessages((current) =>
      current.map((message) => {
        if (message.id !== messageId) return message
        return {
          ...message,
          clientNonce: nextHandle.clientNonce,
          clientStatus: 'sending',
          clientError: undefined,
        }
      })
    )
    return nextHandle
  }, [commitMessages])

  return {
    messages,
    loading,
    error,
    lastReadSeq,
    loadedTarget,
    refresh: fetchHistory,
    reportVisibleSeq,
    addOptimisticMessage,
    ackOptimisticMessage,
    failOptimisticMessage,
    retryOptimisticMessage,
  }
}
