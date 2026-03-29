import { useCallback, useEffect, useRef, useState } from 'react'
import { getHistory, getHistoryAfter, updateReadCursor } from '../api'
import {
  applyRealtimeEvent,
  historyFetchAfterForNotification,
  maxHistorySeq,
  mergeHistoryMessages,
  nextRealtimeCursor,
  resolveRealtimeTarget,
} from '../transport/realtime'
import { getRealtimeSession } from '../transport/realtimeSession'
import type { HistoryMessage, HistoryResponse, RealtimeMessage } from '../types'

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

export function useHistory(username: string, target: string | null) {
  const [messages, setMessages] = useState<HistoryMessage[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [lastReadSeq, setLastReadSeq] = useState(0)
  const [loadedTarget, setLoadedTarget] = useState<string | null>(null)
  const lastEventIdRef = useRef(0)
  const streamIdRef = useRef<string | null>(null)
  const lastStreamPosRef = useRef(0)
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
    if (!username || !target) return null
    if (after != null && incrementalFetchAfterRef.current === after) {
      return null
    }
    if (after != null) {
      incrementalFetchAfterRef.current = after
    }
    try {
      const res =
        after != null
          ? await getHistoryAfter(username, target, after, 50)
          : await getHistory(username, target, 50)
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
      setLoadedTarget(target)
      lastEventIdRef.current = Math.max(lastEventIdRef.current, res.latestEventId ?? 0)
      streamIdRef.current = res.streamId ?? streamIdRef.current
      lastStreamPosRef.current = Math.max(lastStreamPosRef.current, res.streamPos ?? 0)
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
  }, [username, target])

  useEffect(() => {
    if (!username || !target) {
      setMessages([])
      setError(null)
      setLastReadSeq(0)
      setLoadedTarget(null)
      lastEventIdRef.current = 0
      streamIdRef.current = null
      lastStreamPosRef.current = 0
      maxLoadedSeqRef.current = 0
      incrementalFetchAfterRef.current = null
      return
    }

    let cancelled = false
    let unsubscribeRealtime: (() => void) | null = null
    let activeRealtimeTarget: string | null = null
    const activeTarget = target

    async function bootstrap() {
      setLoading(true)
      setMessages([])
      setError(null)
      setLastReadSeq(0)
      setLoadedTarget(null)
      lastEventIdRef.current = 0
      streamIdRef.current = null
      lastStreamPosRef.current = 0
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
        activeRealtimeTarget = await resolveRealtimeTarget(username, activeTarget)
      } catch (targetError) {
        if (!cancelled) {
          setError(targetError instanceof Error ? targetError.message : String(targetError))
        }
        return
      }

      unsubscribeRealtime = getRealtimeSession(username).subscribe({
        targets: [activeRealtimeTarget],
        resumeFrom: lastEventIdRef.current,
        streamId: streamIdRef.current,
        resumeFromStreamPos: lastStreamPosRef.current,
        onFrame: (frame: RealtimeMessage) => {
          if (cancelled) return
          if (frame.type === 'subscribed') {
            lastEventIdRef.current = nextRealtimeCursor(lastEventIdRef.current, frame)
            if (frame.streamId) {
              streamIdRef.current = frame.streamId
              lastStreamPosRef.current = frame.resumeFromStreamPos ?? lastStreamPosRef.current
            }
            return
          }
          if (frame.type === 'error') {
            setError(frame.message)
            return
          }
          lastEventIdRef.current = nextRealtimeCursor(lastEventIdRef.current, frame)
          if (streamIdRef.current && frame.event.streamId === streamIdRef.current) {
            lastStreamPosRef.current = Math.max(
              lastStreamPosRef.current,
              frame.event.streamPos ?? 0
            )
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
          if (frame.event.eventType === 'conversation.state' || frame.event.eventType === 'thread.state') {
            return
          }
          if (
            frame.event.eventType === 'conversation.read_cursor_set' ||
            frame.event.eventType === 'thread.read_cursor_set'
          ) {
            const nextReadSeq = frame.event.payload.lastReadSeq
            if (typeof nextReadSeq === 'number') {
              setLastReadSeq((current) => Math.max(current, nextReadSeq))
            }
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
  }, [fetchHistory, target, username])

  const reportVisibleSeq = useCallback((visibleSeq: number) => {
    if (!username || !target || visibleSeq <= 0) return
    if (loadedTarget !== target) return
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
        await updateReadCursor(username, target, flushSeq)
        setLastReadSeq((current) => Math.max(current, flushSeq))
      } catch (cursorError) {
        console.error('Failed to update read cursor', cursorError)
      }
    }, 150)
  }, [loadedTarget, target, username])

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
