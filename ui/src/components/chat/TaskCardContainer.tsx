import { useState } from 'react'
import { useTask } from '../../hooks/useTask'
import {
  claimTasks,
  unclaimTask,
  updateTaskStatus,
} from '../../data/tasks'
import { TaskCard, type TaskAction } from './TaskCard'
import { formatTime, senderColor } from './MessageItem'

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

  // Render the card inside a regular message-item shell so it visually flows
  // as just another message in the chat stream — matches the convention every
  // other row uses (avatar + sender + timestamp + body) rather than reading as
  // a floating popover panel.
  const senderName = task.createdBy
  const initial = senderName[0]?.toUpperCase() ?? 'T'
  return (
    <div className="message-item message-task">
      <div
        className="message-avatar"
        style={{ background: senderColor(senderName) }}
      >
        <span style={{ fontSize: 12, fontWeight: 700 }}>{initial}</span>
      </div>
      <div className="message-body">
        <div className="message-header">
          <span className="message-sender">{senderName}</span>
          <span className="message-status">opened task #{task.taskNumber}</span>
          <span
            className="message-time"
            title={new Date(task.createdAt).toLocaleString()}
          >
            {formatTime(task.createdAt)}
          </span>
        </div>
        <TaskCard task={task} onAction={dispatch} busy={busy} />
      </div>
    </div>
  )
}
