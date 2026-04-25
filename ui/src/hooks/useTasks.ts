import { useState, useEffect, useCallback } from 'react'
import { getTasks } from '../data'
import type { TaskInfo } from '../data'
import { useTasksStore } from '../store/tasksStore'

/**
 * Polls task list for a conversation and pushes the result into the global
 * tasks slice so unrelated consumers (TaskCard host messages, useTask) can
 * read the same row without re-fetching. The local `tasks` array stays for
 * legacy consumers that want a stable order — but state mutations route
 * through `applyUpdate` on the realtime stream.
 */
export function useTasks(username: string, conversationId: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([])
  const [loading, setLoading] = useState(false)
  const setAll = useTasksStore((s) => s.setAll)

  const fetchTasks = useCallback(async () => {
    if (!username || !conversationId) return
    try {
      const res = await getTasks(conversationId, 'all')
      setTasks(res.tasks)
      setAll(res.tasks)
    } catch (e) {
      console.error('fetchTasks error', e)
    } finally {
      setLoading(false)
    }
  }, [conversationId, username, setAll])

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
