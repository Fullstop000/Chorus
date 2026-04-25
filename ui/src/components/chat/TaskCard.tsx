import type { TaskInfo } from '../../data/tasks'
import './TaskCard.css'

/**
 * Closed action set the host (MessageList) wires into HTTP calls. The card
 * never decides what URL to hit — it just declares what the user wants and
 * the container chooses the right `data/tasks.ts` helper. Keeps the card
 * pure-presentational and trivially unit-testable.
 */
export type TaskAction =
  | { kind: 'accept' }
  | { kind: 'dismiss' }
  | { kind: 'claim' }
  | { kind: 'unclaim' }
  | { kind: 'start' }
  | { kind: 'sendForReview' }
  | { kind: 'markDone' }
  | { kind: 'openSubChannel' }

export interface TaskCardProps {
  task: TaskInfo
  onAction: (action: TaskAction) => void
  /** True while an action is in flight — disables every CTA in the card. */
  busy: boolean
}

/**
 * Single render of the unified task lifecycle for the parent-channel host
 * message. Six branches, each driven by `task.status` plus (for `todo`) the
 * `owner` shape:
 *
 *   proposed              → accept / dismiss
 *   dismissed             → muted, terminal, no CTA
 *   todo (no owner)       → claim
 *   todo (with owner)     → start (claim already implicit in seed)
 *   in_progress           → sendForReview + sub-channel deep-link
 *   in_review             → markDone + sub-channel deep-link
 *   done                  → collapsed pill, link only
 *
 * State swap is React-driven, not CSS-animated — the older `TaskEventMessage`
 * morph was tied to the per-task event log and is dropped here in favor of
 * a flat re-render on every update.
 */
export function TaskCard({ task, onAction, busy }: TaskCardProps) {
  const testId = `task-card-${task.taskNumber}`

  if (task.status === 'proposed') {
    return (
      <div className="task-card" data-testid={testId} data-status="proposed">
        <header className="task-card__head">
          <span className="task-card__num">#{task.taskNumber}</span>
          <span className="task-card__status" data-status="proposed">
            proposed
          </span>
        </header>
        <div className="task-card__title">{task.title}</div>
        {hasSnapshot(task) && (
          <blockquote className="task-card__snapshot">
            <span className="task-card__snapshot-source">
              {task.snapshotSenderName ?? 'unknown'}
            </span>
            <span className="task-card__snapshot-content">
              {task.snapshotContent ?? ''}
            </span>
          </blockquote>
        )}
        <div className="task-card__actions">
          <button
            type="button"
            data-testid="task-card-accept-btn"
            className="task-card__btn task-card__btn--primary"
            onClick={() => onAction({ kind: 'accept' })}
            disabled={busy}
          >
            create
          </button>
          <button
            type="button"
            data-testid="task-card-dismiss-btn"
            className="task-card__btn"
            onClick={() => onAction({ kind: 'dismiss' })}
            disabled={busy}
          >
            dismiss
          </button>
        </div>
      </div>
    )
  }

  if (task.status === 'dismissed') {
    return (
      <div className="task-card task-card--muted" data-testid={testId} data-status="dismissed">
        <header className="task-card__head">
          <span className="task-card__num">#{task.taskNumber}</span>
          <span className="task-card__status" data-status="dismissed">
            dismissed
          </span>
        </header>
        <div className="task-card__title task-card__title--strike">{task.title}</div>
      </div>
    )
  }

  if (task.status === 'todo') {
    const claimed = !!task.owner
    return (
      <div className="task-card" data-testid={testId} data-status="todo" data-claimed={claimed ? 'true' : 'false'}>
        <header className="task-card__head">
          <span className="task-card__num">#{task.taskNumber}</span>
          <span className="task-card__status" data-status="todo">
            todo
          </span>
        </header>
        <div className="task-card__title">{task.title}</div>
        <div className="task-card__meta">
          {claimed ? (
            <span className="task-card__owner">claimed by @{task.owner}</span>
          ) : (
            <span className="task-card__owner task-card__owner--unowned">unowned</span>
          )}
        </div>
        <div className="task-card__actions">
          {claimed ? (
            <button
              type="button"
              data-testid="task-card-start-btn"
              className="task-card__btn task-card__btn--primary"
              onClick={() => onAction({ kind: 'start' })}
              disabled={busy}
            >
              start
            </button>
          ) : (
            <button
              type="button"
              data-testid="task-card-claim-btn"
              className="task-card__btn task-card__btn--primary"
              onClick={() => onAction({ kind: 'claim' })}
              disabled={busy}
            >
              claim
            </button>
          )}
        </div>
      </div>
    )
  }

  if (task.status === 'in_progress') {
    return (
      <div className="task-card" data-testid={testId} data-status="in_progress">
        <header className="task-card__head">
          <span className="task-card__num">#{task.taskNumber}</span>
          <span className="task-card__status" data-status="in_progress">
            in progress
          </span>
        </header>
        <div className="task-card__title">{task.title}</div>
        <div className="task-card__meta">
          {task.owner && <span className="task-card__owner">@{task.owner}</span>}
          {task.subChannelId && (
            <button
              type="button"
              data-testid="task-card-link"
              className="task-card__link"
              onClick={() => onAction({ kind: 'openSubChannel' })}
            >
              open thread →
            </button>
          )}
        </div>
        <div className="task-card__actions">
          <button
            type="button"
            data-testid="task-card-review-btn"
            className="task-card__btn task-card__btn--primary"
            onClick={() => onAction({ kind: 'sendForReview' })}
            disabled={busy}
          >
            send for review
          </button>
        </div>
      </div>
    )
  }

  if (task.status === 'in_review') {
    return (
      <div className="task-card" data-testid={testId} data-status="in_review">
        <header className="task-card__head">
          <span className="task-card__num">#{task.taskNumber}</span>
          <span className="task-card__status" data-status="in_review">
            in review
          </span>
        </header>
        <div className="task-card__title">{task.title}</div>
        <div className="task-card__meta">
          {task.owner && <span className="task-card__owner">@{task.owner}</span>}
          {task.subChannelId && (
            <button
              type="button"
              data-testid="task-card-link"
              className="task-card__link"
              onClick={() => onAction({ kind: 'openSubChannel' })}
            >
              open thread →
            </button>
          )}
        </div>
        <div className="task-card__actions">
          <button
            type="button"
            data-testid="task-card-done-btn"
            className="task-card__btn task-card__btn--primary"
            onClick={() => onAction({ kind: 'markDone' })}
            disabled={busy}
          >
            mark done
          </button>
        </div>
      </div>
    )
  }

  // status === 'done'
  return (
    <div
      className="task-card task-card--pill"
      data-testid={testId}
      data-status="done"
    >
      <button
        type="button"
        data-testid="task-card-done-pill"
        className="task-card__done-pill"
        onClick={() => onAction({ kind: 'openSubChannel' })}
      >
        <span className="task-card__done-tag">done</span>
        <span className="task-card__num">#{task.taskNumber}</span>
        <span className="task-card__title task-card__title--strike">{task.title}</span>
        <span className="task-card__done-open">open →</span>
      </button>
    </div>
  )
}

/** Treat "any snapshot field present" as enough to render the provenance quote. */
function hasSnapshot(task: TaskInfo): boolean {
  return !!(
    task.snapshotContent ??
    task.snapshotSenderName ??
    task.snapshotCreatedAt
  )
}
