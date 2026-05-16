import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useStore } from "../../store";
import { tasksBoardPath } from "../../lib/routes";
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
 * Display label for the status badge. The API returns the enum verbatim
 * (`in_progress`, `in_review`) which reads as code. Map to space-separated
 * words so the badge matches the board column headers ("in progress",
 * "in review") and the advance button vocabulary.
 */
const STATUS_LABEL: Record<TaskStatus, string> = {
  todo: "todo",
  in_progress: "in progress",
  in_review: "in review",
  done: "done",
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
  // Show the advance button for every non-terminal status so the workflow is
  // always legible. When the current user cannot advance (claimed by someone
  // else, or legacy pre-backfill row), render it disabled with an explanatory
  // title — hiding it silently leaves non-claimers with no signal about who
  // owns the task or why they can't move it.
  const showAdvance = !!task && !!advanceLabel;
  const hasSubChannel =
    !!task && task.subChannelId !== null && task.subChannelId !== undefined;
  const advanceDisabled = advancing || !hasSubChannel || !canAdvance;
  // Priority: legacy failure first (the more surprising blocker), then
  // permission. Both titles are displayed on hover via the native `title`.
  let advanceTitle: string | undefined;
  if (!hasSubChannel) {
    advanceTitle =
      "This task was created before sub-channels existed and cannot be advanced. Create a new task to collaborate.";
  } else if (!canAdvance && task?.claimedByName) {
    advanceTitle = `Only ${task.claimedByName} can advance this task.`;
  }

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
            <span className="task-detail__status">{STATUS_LABEL[task.status]}</span>
            {task.claimedByName && (
              <span>claimed by {task.claimedByName}</span>
            )}
            <span>created by {task.createdByName ?? "unknown"}</span>
            {showAdvance && (
              <button
                type="button"
                className="task-detail__advance"
                onClick={onAdvance}
                disabled={advanceDisabled}
                title={advanceTitle}
                aria-disabled={advanceDisabled || undefined}
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
  const { currentUser, currentUserId, currentTaskDetail } = useStore();
  const navigate = useNavigate();
  const location = useLocation();
  const [task, setTask] = useState<TaskInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [advanceError, setAdvanceError] = useState<string | null>(null);
  const [advancing, setAdvancing] = useState(false);

  // Bump when we want to force-refetch the task (e.g. after a successful
  // status transition). Including it in the effect deps keeps fetch logic in
  // one place instead of duplicating it inside the advance handler.
  const [refreshTick, setRefreshTick] = useState(0);

  // Single close path — used by the back button and by Esc. Returns the user
  // to wherever they came from (recorded in location.state.from by the
  // caller that opened the detail). Falls back to the parent channel's
  // tasks board when there's no recorded origin (e.g. deep-link entry).
  // Not `navigate(-1)` — history.length lies in SPAs.
  function handleClose() {
    const from = (location.state as { from?: string } | null)?.from
    if (from) {
      navigate(from)
      return
    }
    if (currentTaskDetail) {
      navigate(tasksBoardPath(currentTaskDetail.parentSlug), { replace: true })
    }
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

  // Stable identity key for the current task target. Used to distinguish a
  // "switching to a different task" change (where we should wipe stale state
  // so users don't see the previous task's title/status/chat flash) from a
  // "refreshing the same task after an advance" change (where the stale
  // display is the correct display until the fetch resolves).
  const targetKey = currentTaskDetail
    ? `${currentTaskDetail.parentChannelId}#${currentTaskDetail.taskNumber}`
    : null;

  // Wipe task + per-task error state the instant the target changes. Runs
  // before the fetch effect so the UI flips to the loading state immediately.
  useEffect(() => {
    setTask(null);
    setError(null);
    setAdvanceError(null);
    setAdvancing(false);
  }, [targetKey]);

  useEffect(() => {
    if (!currentTaskDetail) {
      return;
    }
    let alive = true;
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
  const history = useHistory(currentUserId, subChannelName, subChannelId);

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

  // Legacy rows predate the task=sub-channel primitive: backfill tried to
  // spawn a child channel and for some reason didn't. The chat surface has
  // nothing to render, and MessageInput would have no target, so we replace
  // both with a visible explanation instead of leaking a generic "select a
  // channel" empty state from ChatPanel.
  const hasLoadedTask = !!task;
  const hasSubChannel =
    hasLoadedTask && subChannelId !== null && subChannelName !== null;

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
      {hasLoadedTask && !hasSubChannel ? (
        <div className="task-detail__legacy-notice" role="note">
          <p>
            This task predates the task = sub-channel primitive and has no
            collaboration surface.
          </p>
          <p>Create a new task to discuss or hand it off.</p>
        </div>
      ) : (
        <>
          <div className="task-detail__body">
            <ChatPanel
              target={subChannelName}
              conversationId={subChannelId}
              messages={history.messages}
              loading={history.loading}
              lastReadSeq={history.lastReadSeq}
              emptyLabel="No updates on this task yet. Post the first one below."
            />
          </div>
          {subChannelId && subChannelName && (
            <MessageInput
              target={subChannelName}
              conversationId={subChannelId}
              history={history}
              hideCreateTaskCheckbox
              placeholder={`Message task #${currentTaskDetail.taskNumber}`}
            />
          )}
        </>
      )}
    </div>
  );
}
