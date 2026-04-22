import { useEffect, useState } from "react";
import { useStore } from "../../store";
import type { TaskDetailTarget } from "../../store/uiStore";
import { useHistory } from "../../hooks/useHistory";
import { ChatPanel } from "../chat/ChatPanel";
import { MessageInput } from "../chat/MessageInput";
import { getTaskDetail, type TaskInfo } from "../../data";
import "./TaskDetail.css";

interface TaskDetailViewProps {
  target: TaskDetailTarget;
  task: TaskInfo | null;
  error: string | null;
  onBack: () => void;
}

/**
 * Pure presentational header + status surface for the task-detail view.
 * Split from the store/data-bound container so tests can render it without
 * spinning up Zustand, fetch, or the realtime session. Chat body stays in
 * the container because it depends on hooks (`useHistory`) that can't be
 * exercised under `renderToStaticMarkup`.
 */
export function TaskDetailView({
  target,
  task,
  error,
  onBack,
}: TaskDetailViewProps) {
  return (
    <header className="task-detail__header">
      <button
        type="button"
        onClick={onBack}
        aria-label="back to channel"
        className="task-detail__back"
      >
        ← back
      </button>
      <div className="task-detail__breadcrumb">
        <span>{target.parentSlug}</span>
        <span aria-hidden="true"> · </span>
        <span>task #{target.taskNumber}</span>
      </div>
      {task ? (
        <>
          <h1 className="task-detail__title">{task.title}</h1>
          <div className="task-detail__meta">
            <span className="task-detail__status">{task.status}</span>
            {task.claimedByName && (
              <span>claimed by {task.claimedByName}</span>
            )}
            <span>created by {task.createdByName ?? "unknown"}</span>
          </div>
        </>
      ) : error ? (
        <div className="task-detail__error">Failed to load task: {error}</div>
      ) : (
        <div className="task-detail__loading">Loading…</div>
      )}
    </header>
  );
}

/**
 * Container that fetches the task, subscribes to the sub-channel's message
 * history, and renders the header + embedded chat.
 *
 * Hook-call ordering note: `useHistory` is called unconditionally with
 * (potentially null) `subChannelName` and `subChannelId`. The hook is
 * designed to no-op when either is null (`enabled` gate on the useQuery),
 * so the hook call is stable across renders and satisfies Rules of Hooks.
 */
export function TaskDetail() {
  const { currentUser, currentTaskDetail, setCurrentTaskDetail } = useStore();
  const [task, setTask] = useState<TaskInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!currentTaskDetail) {
      setTask(null);
      setError(null);
      return;
    }
    let alive = true;
    setTask(null);
    setError(null);
    getTaskDetail(currentTaskDetail.parentChannelId, currentTaskDetail.taskNumber)
      .then((t) => {
        if (!alive) return;
        setTask(t);
      })
      .catch((e) => {
        if (!alive) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [currentTaskDetail]);

  // Hooks must run unconditionally — pull sub-channel wiring before any early
  // return. `useHistory` tolerates null args and stays idle until both are set.
  const subChannelName = task?.subChannelName ?? null;
  const subChannelId = task?.subChannelId ?? null;
  const history = useHistory(currentUser, subChannelName, subChannelId);

  if (!currentTaskDetail) return null;

  return (
    <div data-testid="task-detail" className="task-detail">
      <TaskDetailView
        target={currentTaskDetail}
        task={task}
        error={error}
        onBack={() => setCurrentTaskDetail(null)}
      />
      <div className="task-detail__body">
        <ChatPanel
          target={subChannelName}
          conversationId={subChannelId}
          messages={history.messages}
          loading={history.loading}
          lastReadSeq={history.lastReadSeq}
        />
      </div>
      {subChannelId && subChannelName && (
        <MessageInput
          target={subChannelName}
          conversationId={subChannelId}
          history={history}
          hideCreateTaskCheckbox
        />
      )}
    </div>
  );
}
