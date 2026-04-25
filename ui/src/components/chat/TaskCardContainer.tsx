import { useState } from 'react'
import { useTask } from '../../hooks/useTask'
import {
  claimTasks,
  unclaimTask,
  updateTaskStatus,
} from '../../data/tasks'
import { TaskCard, type TaskAction } from './TaskCard'

export interface TaskCardWirePayload {
  kind: 'task_card'
  taskId: string
  taskNumber: number
  // Fields below are present on the wire but the container reads the live
  // task from the tasks store via `useTask` rather than the snapshot, so the
  // card always reflects the latest realtime state.
  [key: string]: unknown
}

interface TaskCardContainerProps {
  /** Parsed `task_card` payload from `messages.content`. */
  payload: TaskCardWirePayload
  /**
   * Parent channel id — every CTA hits the public conversation routes scoped
   * to this id. Required because the task may have been carved into a
   * sub-channel; without it we'd have to look it up per-action.
   */
  parentChannelId: string
  /**
   * Hook surface for the openSubChannel action. The chat panel knows how to
   * navigate; this container only knows the action happened.
   */
  onOpenSubChannel?: (subChannelId: string, subChannelName: string | null) => void
}

/**
 * Bridge between the parent-channel host message and the live task. Pulls
 * the row out of the tasks store via `useTask` (Rules of Hooks: cannot be
 * called inside MessageList's render loop), wires every CTA into the
 * appropriate `data/tasks.ts` helper, and surfaces a transient `busy` flag
 * so users can't double-fire actions.
 *
 * If the row is unknown (e.g. the parent channel hasn't fetched its task list
 * yet), renders nothing — the next list fetch or `task_update` frame will
 * populate it.
 */
export function TaskCardContainer({
  payload,
  parentChannelId,
  onOpenSubChannel,
}: TaskCardContainerProps) {
  const task = useTask(payload.taskId)
  const [busy, setBusy] = useState(false)

  if (!task) return null

  async function dispatch(action: TaskAction) {
    if (!task) return
    if (action.kind === 'openSubChannel') {
      if (task.subChannelId) {
        onOpenSubChannel?.(task.subChannelId, task.subChannelName ?? null)
      }
      return
    }
    setBusy(true)
    try {
      switch (action.kind) {
        case 'accept':
          await updateTaskStatus(parentChannelId, task.taskNumber, 'todo')
          return
        case 'dismiss':
          await updateTaskStatus(parentChannelId, task.taskNumber, 'dismissed')
          return
        case 'claim':
          await claimTasks(parentChannelId, [task.taskNumber])
          return
        case 'unclaim':
          await unclaimTask(parentChannelId, task.taskNumber)
          return
        case 'start':
          await updateTaskStatus(parentChannelId, task.taskNumber, 'in_progress')
          return
        case 'sendForReview':
          await updateTaskStatus(parentChannelId, task.taskNumber, 'in_review')
          return
        case 'markDone':
          await updateTaskStatus(parentChannelId, task.taskNumber, 'done')
          return
      }
    } catch (err) {
      // Surface but don't swallow — the UI doesn't have a toast surface here
      // yet, so log loudly. Callers wanting in-card error UI should lift the
      // error state out of this container.
      console.error('[TaskCard] action failed', action.kind, err)
    } finally {
      setBusy(false)
    }
  }

  return <TaskCard task={task} onAction={dispatch} busy={busy} />
}
