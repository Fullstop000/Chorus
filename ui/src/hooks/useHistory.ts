import { useCallback, useEffect, useRef, useState } from 'react'
import { getHistory } from '../api'
import {
  applyRealtimeEvent,
  createRealtimeSocket,
  nextRealtimeCursor,
  resolveRealtimeScope,
} from '../transport/realtime'
import type { HistoryMessage, HistoryResponse, RealtimeMessage, RealtimeScope } from '../types'

export function useHistory(username: string, target: string | null) {
  const [messages, setMessages] = useState<HistoryMessage[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const lastEventIdRef = useRef(0)
  const streamIdRef = useRef<string | null>(null)
  const lastStreamPosRef = useRef(0)

  const fetchHistory = useCallback(async (): Promise<HistoryResponse | null> => {
    if (!username || !target) return null
    try {
      const res = await getHistory(username, target, 50)
      setMessages(res.messages)
      lastEventIdRef.current = res.latestEventId ?? 0
      streamIdRef.current = res.streamId ?? null
      lastStreamPosRef.current = res.streamPos ?? 0
      setError(null)
      return res
    } catch (e) {
      setError(String(e))
      return null
    } finally {
      setLoading(false)
    }
  }, [username, target])

  useEffect(() => {
    if (!username || !target) {
      setMessages([])
      setError(null)
      lastEventIdRef.current = 0
      streamIdRef.current = null
      lastStreamPosRef.current = 0
      return
    }

    let cancelled = false
    let reconnectTimer: number | null = null
    let socket: WebSocket | null = null
    let activeScope: RealtimeScope | null = null
    const activeTarget = target

    const connect = () => {
      if (cancelled || !activeScope) return

      socket = createRealtimeSocket(username)
      socket.onopen = () => {
        const subscribeFrame: Record<string, unknown> = {
          type: 'subscribe',
          resumeFrom: lastEventIdRef.current,
          scopes: [activeScope],
        }
        if (streamIdRef.current) {
          subscribeFrame.streamId = streamIdRef.current
          subscribeFrame.resumeFromStreamPos = lastStreamPosRef.current
        }
        socket?.send(
          JSON.stringify(subscribeFrame)
        )
      }
      socket.onmessage = (messageEvent) => {
        try {
          const frame = JSON.parse(String(messageEvent.data)) as RealtimeMessage
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
          setMessages((current) => applyRealtimeEvent(current, frame.event))
          setError(null)
        } catch (eventError) {
          console.error('Failed to parse realtime frame', eventError)
        }
      }
      socket.onerror = () => {
        socket?.close()
      }
      socket.onclose = () => {
        if (cancelled) return
        reconnectTimer = window.setTimeout(connect, 1_000)
      }
    }

    async function bootstrap() {
      setLoading(true)
      setMessages([])
      setError(null)
      lastEventIdRef.current = 0
      streamIdRef.current = null
      lastStreamPosRef.current = 0

      const history = await fetchHistory()
      if (cancelled || !history) return

      try {
        activeScope = await resolveRealtimeScope(username, activeTarget)
      } catch (scopeError) {
        if (!cancelled) {
          setError(scopeError instanceof Error ? scopeError.message : String(scopeError))
        }
        return
      }

      connect()
    }

    void bootstrap()

    return () => {
      cancelled = true
      if (reconnectTimer != null) window.clearTimeout(reconnectTimer)
      socket?.close()
    }
  }, [fetchHistory, target, username])

  return { messages, loading, error, refresh: fetchHistory }
}
