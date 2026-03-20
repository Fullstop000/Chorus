import { useState, useEffect, useCallback } from 'react'
import { getTasks } from '../api'
import type { TaskInfo } from '../types'

export function useTasks(username: string, channel: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([])
  const [loading, setLoading] = useState(false)

  const fetchTasks = useCallback(async () => {
    if (!username || !channel) return
    try {
      const res = await getTasks(username, channel, 'all')
      setTasks(res.tasks)
    } catch (e) {
      console.error('fetchTasks error', e)
    } finally {
      setLoading(false)
    }
  }, [username, channel])

  useEffect(() => {
    if (!channel) {
      setTasks([])
      return
    }
    setLoading(true)
    fetchTasks()
    const id = setInterval(fetchTasks, 5_000)
    return () => clearInterval(id)
  }, [channel, fetchTasks])

  return { tasks, loading, refresh: fetchTasks }
}
