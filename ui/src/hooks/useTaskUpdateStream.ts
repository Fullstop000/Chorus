import { useEffect } from 'react'
import { getSession } from '../transport/session'
import { useTasksStore } from '../store/tasksStore'
import type { TaskStatus } from '../data/tasks'

/**
 * Mounted once at the app root. Subscribes to cross-channel `task_update`
 * frames and patches the tasks slice in place. Wire `status` is widened to
 * `TaskStatus` here at the boundary — the transport layer keeps it as
 * `string` so it can ferry future enum additions without a type cast every
 * frame.
 */
export function useTaskUpdateStream(viewer: string | null) {
  const applyUpdate = useTasksStore((s) => s.applyUpdate)

  useEffect(() => {
    if (!viewer) return
    const session = getSession(viewer)
    return session.subscribeTaskUpdates((frame) => {
      applyUpdate({
        taskId: frame.taskId,
        status: frame.status as TaskStatus,
        owner: frame.owner,
        subChannelId: frame.subChannelId,
        updatedAt: frame.updatedAt,
      })
    })
  }, [viewer, applyUpdate])
}
