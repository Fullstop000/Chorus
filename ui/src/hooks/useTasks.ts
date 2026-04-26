import { useState, useEffect, useCallback } from 'react'
import { getTasks } from '../data'
import type { TaskInfo } from '../data'

export function useTasks(viewerHumanId: string, conversationId: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([])
  const [loading, setLoading] = useState(false)

  const fetchTasks = useCallback(async () => {
    if (!viewerHumanId || !conversationId) return
    try {
      const res = await getTasks(conversationId, 'all')
      setTasks(res.tasks)
    } catch (e) {
      console.error('fetchTasks error', e)
    } finally {
      setLoading(false)
    }
  }, [conversationId, viewerHumanId])

  useEffect(() => {
    if (!conversationId) {
      setTasks([])
      return
    }
    setLoading(true)
    fetchTasks()
    const id = setInterval(fetchTasks, 5_000)
    return () => clearInterval(id)
  }, [conversationId, fetchTasks])

  return { tasks, loading, refresh: fetchTasks }
}
