import { useStore } from "../../store";
import type { TaskDetailTarget } from "../../store/uiStore";

interface TaskDetailViewProps {
  target: TaskDetailTarget;
  onBack: () => void;
}

/**
 * Pure presentational body of the task-detail view. Split from the
 * store-bound wrapper so tests can render it without going through
 * Zustand's SSR snapshot (which pins to initial state).
 */
export function TaskDetailView({ target, onBack }: TaskDetailViewProps) {
  return (
    <div data-testid="task-detail" className="task-detail">
      <button
        type="button"
        onClick={onBack}
        aria-label="back to channel"
        className="task-detail-back"
      >
        ← back
      </button>
      <h2 className="task-detail-title">
        {target.parentSlug} · task #{target.taskNumber}
      </h2>
      <p className="task-detail-placeholder">
        Task detail view — full content in Task 9.
      </p>
    </div>
  );
}

/**
 * Placeholder task-detail container. Task 9 replaces the body with the
 * embedded sub-channel chat; for now it only renders breadcrumbs + a back
 * affordance so navigation (Task 8) can be wired up independently.
 */
export function TaskDetail() {
  const { currentTaskDetail, setCurrentTaskDetail } = useStore();
  if (!currentTaskDetail) return null;
  return (
    <TaskDetailView
      target={currentTaskDetail}
      onBack={() => setCurrentTaskDetail(null)}
    />
  );
}
