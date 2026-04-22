import { useEffect, useState } from "react";
import { useStore } from "../../store";
import type { TaskDetailTarget } from "../../store/uiStore";
import { useHistory } from "../../hooks/useHistory";
import { ChatPanel } from "../chat/ChatPanel";
import { MessageInput } from "../chat/MessageInput";
import {
  claimTasks,
  getTaskDetail,
  updateTaskStatus,
  type TaskInfo,
  type TaskStatus,
} from "../../data";
import "./TaskDetail.css";

/**
 * Next legal status for the "advance" affordance, in the Todo → InProgress →
 * InReview → Done kanban chain. `null` means no further advancement (Done).
 */
const NEXT_STATUS: Record<TaskStatus, TaskStatus | null> = {
  todo: "in_progress",
  in_progress: "in_review",
  in_review: "done",
  done: null,
};

/**
 * Human label for the advance button for each current status. Todo is phrased
 * as "Start" because advancing from Todo also claims the task on the user's
 * behalf (claim + status bump in one click).
 */
const ADVANCE_LABEL: Record<TaskStatus, string | null> = {
  todo: "Start",
  in_progress: "Submit for review",
  in_review: "Mark done",
  done: null,
};

/**
 * Pure permission check: can `currentUser` advance `task`? Claim-on-start is
 * allowed for anyone when the task is unclaimed Todo; subsequent transitions
 * require the current user to be the claimer. Mirrors the historical
 * `TaskCard.advance()` logic from pre-Task-8 TasksPanel.
 */
function canAdvanceTask(task: TaskInfo, currentUser: string): boolean {
  if (task.status === "done") return false;
  if (task.status === "todo") {
    return !task.claimedByName || task.claimedByName === currentUser;
  }
  return task.claimedByName === currentUser;
}

interface TaskDetailViewProps {
  target: TaskDetailTarget;
  task: TaskInfo | null;
  error: string | null;
  advanceError: string | null;
  advancing: boolean;
  onBack: () => void;
  onAdvance: () => void;
  canAdvance: boolean;
  advanceLabel: string | null;
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
  advanceError,
  advancing,
  onBack,
  onAdvance,
  canAdvance,
  advanceLabel,
}: TaskDetailViewProps) {
  // Hide the advance button when sub_channel_id is null (legacy pre-backfill
  // tasks) — those rows predate the sub-channel machinery and should be read-only
  // in the detail view. Also gate on advanceLabel + canAdvance.
  const showAdvance =
    !!task &&
    !!advanceLabel &&
    canAdvance &&
    task.subChannelId !== null &&
    task.subChannelId !== undefined;

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
            {showAdvance && (
              <button
                type="button"
                className="task-detail__advance"
                onClick={onAdvance}
                disabled={advancing}
              >
                {advancing ? "…" : advanceLabel}
              </button>
            )}
          </div>
          {advanceError && (
            <div className="task-detail__error" role="alert">
              Failed to advance task: {advanceError}
            </div>
          )}
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
  const { currentUser, currentTaskDetail, setCurrentTaskDetail, setActiveTab } =
    useStore();
  const [task, setTask] = useState<TaskInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [advanceError, setAdvanceError] = useState<string | null>(null);
  const [advancing, setAdvancing] = useState(false);

  // Bump when we want to force-refetch the task (e.g. after a successful
  // status transition). Including it in the effect deps keeps fetch logic in
  // one place instead of duplicating it inside the advance handler.
  const [refreshTick, setRefreshTick] = useState(0);

  // Single close path — used by the back button and by Esc. Restores whatever
  // tab the user came from (falls back to Tasks when the target didn't stash one,
  // e.g. deep-linked or programmatic opens).
  function handleClose() {
    setActiveTab(currentTaskDetail?.returnToTab ?? "tasks");
    setCurrentTaskDetail(null);
  }

  // Esc should back out of the detail overlay (keyboard a11y — Dialogs and
  // overlay panels in Chorus use this pattern).
  useEffect(() => {
    if (!currentTaskDetail) return;
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        handleClose();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // handleClose is defined in the same closure; re-registering per change is safe.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentTaskDetail]);

  useEffect(() => {
    if (!currentTaskDetail) {
      setTask(null);
      setError(null);
      return;
    }
    let alive = true;
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
  }, [currentTaskDetail, refreshTick]);

  // Hooks must run unconditionally — pull sub-channel wiring before any early
  // return. `useHistory` tolerates null args and stays idle until both are set.
  const subChannelName = task?.subChannelName ?? null;
  const subChannelId = task?.subChannelId ?? null;
  const history = useHistory(currentUser, subChannelName, subChannelId);

  async function handleAdvance() {
    if (!task || !currentTaskDetail) return;
    const next = NEXT_STATUS[task.status];
    if (!next) return;
    if (!canAdvanceTask(task, currentUser)) return;
    setAdvancing(true);
    setAdvanceError(null);
    try {
      if (task.status === "todo") {
        // Claim implicitly transitions Todo → InProgress server-side, so a
        // single POST covers both steps here.
        await claimTasks(currentTaskDetail.parentChannelId, [task.taskNumber]);
      } else {
        await updateTaskStatus(
          currentTaskDetail.parentChannelId,
          task.taskNumber,
          next,
        );
      }
      setRefreshTick((n) => n + 1);
    } catch (e) {
      setAdvanceError(e instanceof Error ? e.message : String(e));
    } finally {
      setAdvancing(false);
    }
  }

  if (!currentTaskDetail) return null;

  const advanceLabel = task ? ADVANCE_LABEL[task.status] : null;
  const canAdvance = task ? canAdvanceTask(task, currentUser) : false;

  return (
    <div data-testid="task-detail" className="task-detail">
      <TaskDetailView
        target={currentTaskDetail}
        task={task}
        error={error}
        advanceError={advanceError}
        advancing={advancing}
        onBack={handleClose}
        onAdvance={handleAdvance}
        canAdvance={canAdvance}
        advanceLabel={advanceLabel}
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
