import type { TaskEventPayload, TaskEventAction } from '../../data/taskEvents'
import './TaskEventRow.css'

interface TaskEventRowProps {
  /** One parsed task_event row from `useTaskEventLog`. */
  event: TaskEventPayload
  /** Source message id — surfaces as `data-event-id` for tests. */
  eventId: string
  /** ISO8601 createdAt — currently unused in the row body, exposed for tests. */
  createdAt: string
  seq: number
}

/**
 * Inline row for a single task_event message in a sub-channel feed. Replaces
 * the v1 `TaskEventMessage` card-vs-pill thread surface — current task state
 * now lives on the parent-channel `TaskCard`, so sub-channel rows only need
 * to narrate what just happened.
 *
 * The `dismissed` action never fires here per the unified spec
 * (proposed → dismissed re-renders the parent card via task_update; no
 * sub-channel exists yet). The `created` action also doesn't fire — direct
 * create emits a parent-channel `task_card` host message, not a task_event.
 */
export function TaskEventRow({ event, eventId, seq }: TaskEventRowProps) {
  return (
    <div
      className="task-event-row"
      data-action={event.action}
      data-task-number={event.taskNumber}
      data-event-id={eventId}
      data-seq={seq}
      role="listitem"
    >
      <span className="task-event-row__num">#{event.taskNumber}</span>
      <span className="task-event-row__body">{format(event.action, event)}</span>
    </div>
  )
}

function format(action: TaskEventAction, e: TaskEventPayload): string {
  switch (action) {
    case 'claimed':
      return `@${e.actor} claimed`
    case 'unclaimed':
      return `@${e.actor} unclaimed`
    case 'status_changed':
      return `→ ${e.nextStatus.replace('_', ' ')}`
    case 'created':
      // Defensive: legacy chat history may carry pre-T9 `created` rows. Render
      // them as a narrating line rather than dropping silently.
      return `${e.actor} created`
  }
}
