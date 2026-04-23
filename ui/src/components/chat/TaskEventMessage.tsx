import type { TaskState } from '../../hooks/useTaskEventLog'
import type { TaskStatus } from '../../data/tasks'
import './TaskEventMessage.css'

const STATUS_LABEL: Record<TaskStatus, string> = {
  todo: 'todo',
  in_progress: 'in progress',
  in_review: 'in review',
  done: 'done',
}

interface TaskEventMessageProps {
  taskState: TaskState
  /** Invoked when the user clicks the card or the done-pill row. Typically
   *  opens the task detail overlay. */
  onOpen: () => void
}

/**
 * Render one task thread. The outer `data-state` attribute drives the
 * card-vs-pill swap via CSS `max-height` transitions. Prior state is
 * preserved across status changes — React rerenders, CSS animates.
 */
export function TaskEventMessage({
  taskState,
  onOpen,
}: TaskEventMessageProps) {
  const { taskNumber, title, status, claimedBy, events } = taskState
  const isDone = status === 'done'
  return (
    <div
      className="task-event-thread"
      data-state={status}
      data-testid={`task-thread-${taskNumber}`}
    >
      {/* Both card and pill render at all times so the CSS transitions have
          something to animate between. Hide the inactive one from assistive
          tech and remove it from the tab order — max-height:0 + opacity:0
          alone does NOT prevent focus or screen-reader discovery. */}
      <div
        className="task-event-card-view"
        aria-hidden={isDone || undefined}
      >
        <button
          type="button"
          className="task-event-card"
          data-claimed={claimedBy ? 'true' : 'false'}
          onClick={onOpen}
          aria-label={`open task #${taskNumber}`}
          tabIndex={isDone ? -1 : 0}
        >
          <div className="task-event-card-head">
            <span className="task-event-num">#{taskNumber}</span>
            <span className="task-event-status" data-status={status}>
              {STATUS_LABEL[status]}
            </span>
          </div>
          <span className="task-event-title">{title}</span>
          <span className="task-event-meta">
            <span className="task-event-claimer">
              {claimedBy ? `claimed by ${claimedBy}` : ''}
            </span>
          </span>
        </button>
        <div className="task-event-timeline" role="list">
          {events.map((e) => (
            <span
              key={e.eventId}
              className={[
                'task-event-ev',
                'is-show',
                e.nextStatus === 'in_review' && 'is-review',
                e.nextStatus === 'done' && 'is-done',
              ]
                .filter(Boolean)
                .join(' ')}
              role="listitem"
            >
              {formatEvent(e.action, e.actor, e.nextStatus)}
            </span>
          ))}
        </div>
      </div>
      <div
        className="task-event-pill-view"
        aria-hidden={!isDone || undefined}
      >
        <button
          type="button"
          className="task-event-done-row"
          onClick={onOpen}
          aria-label={`open completed task #${taskNumber}`}
          tabIndex={isDone ? 0 : -1}
        >
          <span className="task-event-done-tag">done</span>
          <span>#{taskNumber}</span>
          <span className="task-event-done-strike">{title}</span>
          <span className="task-event-done-open">open →</span>
        </button>
      </div>
    </div>
  )
}

function formatEvent(
  action: TaskState['events'][number]['action'],
  actor: string,
  nextStatus: TaskStatus,
): string {
  switch (action) {
    case 'created':
      return `${actor} created`
    case 'claimed':
      return `${actor} claimed`
    case 'unclaimed':
      return `${actor} unclaimed`
    case 'status_changed':
      return `→ ${STATUS_LABEL[nextStatus]}`
  }
}
