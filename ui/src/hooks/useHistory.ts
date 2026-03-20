import { useState, useEffect, useRef, useCallback } from 'react'
import { getHistory } from '../api'
import type { HistoryMessage } from '../types'

export function useHistory(username: string, target: string | null) {
  const [messages, setMessages] = useState<HistoryMessage[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const lastSeqRef = useRef<number>(0)

  const fetchHistory = useCallback(async () => {
    if (!username || !target) return
    try {
      const res = await getHistory(username, target, 50)
      setMessages(res.messages)
      if (res.messages.length > 0) {
        lastSeqRef.current = res.messages[res.messages.length - 1].seq
      }
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [username, target])

  useEffect(() => {
    if (!target) {
      setMessages([])
      return
    }
    setLoading(true)
    setMessages([])
    lastSeqRef.current = 0
    fetchHistory()
    const id = setInterval(fetchHistory, 2_000)
    return () => clearInterval(id)
  }, [target, fetchHistory])

  return { messages, loading, error, refresh: fetchHistory }
}
