import type { TaskProposalState } from '../../hooks/useTaskProposalLog'
import './TaskProposalMessage.css'

interface TaskProposalMessageProps {
  state: TaskProposalState
  onAccept: () => void
  onDismiss: () => void
  onOpenSubChannel?: () => void
  /** True while a mutation is in-flight — disables both buttons. */
  busy: boolean
}

export function TaskProposalMessage({
  state,
  onAccept,
  onDismiss,
  onOpenSubChannel,
  busy,
}: TaskProposalMessageProps) {
  const { proposalId, status, title, proposedBy, taskNumber, subChannelName } =
    state
  return (
    <div
      className="task-proposal-card"
      data-status={status}
      data-testid={`task-proposal-${proposalId}`}
    >
      <div className="task-proposal-head">
        <span className="task-proposal-kicker">proposed by {proposedBy}</span>
      </div>
      <div className="task-proposal-title">{title}</div>
      {status === 'pending' && (
        <div className="task-proposal-actions">
          <button
            type="button"
            className="task-proposal-btn task-proposal-btn--create"
            onClick={onAccept}
            disabled={busy}
            data-testid="task-proposal-accept-btn"
          >
            create
          </button>
          <button
            type="button"
            className="task-proposal-btn"
            onClick={onDismiss}
            disabled={busy}
            data-testid="task-proposal-dismiss-btn"
          >
            dismiss
          </button>
        </div>
      )}
      {status === 'accepted' && taskNumber !== null && (
        <div className="task-proposal-resolved">
          <span className="task-proposal-resolved-tag">accepted</span>
          <span>
            → task #{taskNumber} opened in{' '}
            <button
              type="button"
              className="task-proposal-link"
              onClick={onOpenSubChannel}
            >
              {subChannelName ?? `task-${taskNumber}`}
            </button>
          </span>
        </div>
      )}
      {status === 'dismissed' && (
        <div className="task-proposal-resolved">
          <span className="task-proposal-resolved-tag task-proposal-resolved-tag--muted">
            dismissed
          </span>
        </div>
      )}
    </div>
  )
}
