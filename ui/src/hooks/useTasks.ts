import { useState, useEffect, useCallback } from 'react'
import { getTasks } from '../api'
import type { TaskInfo } from '../components/tasks/types'

export function useTasks(username: string, conversationId: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([])
  const [loading, setLoading] = useState(false)

  const fetchTasks = useCallback(async () => {
    if (!username || !conversationId) return
    try {
      const res = await getTasks(conversationId, 'all')
      setTasks(res.tasks)
    } catch (e) {
      console.error('fetchTasks error', e)
    } finally {
      setLoading(false)
    }
  }, [conversationId, username])

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
