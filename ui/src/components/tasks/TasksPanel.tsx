import { useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { User } from "lucide-react";
import { useStore } from "../../store";
import { useTasks } from "../../hooks/useTasks";
import { createTasks } from "../../data";
import { taskDetailPath } from "../../lib/routes";
import { useCurrentChannel } from "../../hooks/useRouteSubject";
import type { TaskInfo, TaskStatus } from "./types";
import { FormError } from "@/components/ui/form";
import "./TasksPanel.css";

const COLUMNS: { status: TaskStatus; label: string }[] = [
  { status: "todo", label: "To Do" },
  { status: "in_progress", label: "In Progress" },
  { status: "in_review", label: "In Review" },
  { status: "done", label: "Done" },
];

function TaskCard({
  task,
  parentSlug,
}: {
  task: TaskInfo;
  parentChannelId: string;
  parentSlug: string;
}) {
  const navigate = useNavigate();
  const location = useLocation();

  // Click opens the task detail view at /c/<slug>/tasks/<n>. We carry the
  // current pathname through `location.state.from` so the detail's back
  // button can return the user where they came from (chat origin vs tasks
  // board origin) without relying on `history.length`, which lies in SPAs.
  function openDetail() {
    navigate(taskDetailPath(parentSlug, task.taskNumber), {
      state: { from: location.pathname },
    });
  }

  return (
    <button
      type="button"
      className="task-card"
      onClick={openDetail}
      title={`Open task #${task.taskNumber}`}
    >
      <span className="task-card-number">#{task.taskNumber}</span>
      <span className="task-card-title">{task.title}</span>
      <span className="task-card-meta">
        {task.claimedByName && (
          <span className="task-card-claimed">
            <User size={11} aria-hidden="true" />
            {task.claimedByName}
          </span>
        )}
        {!task.claimedByName && task.createdByName && (
          <span>by {task.createdByName}</span>
        )}
      </span>
    </button>
  );
}

export function TasksPanel() {
  const { currentUserId } = useStore();
  const currentChannel = useCurrentChannel();
  const channelId = currentChannel?.id ?? null;
  const { tasks, loading, refresh } = useTasks(currentUserId, channelId);
  const [newTaskTitle, setNewTaskTitle] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleCreate() {
    if (!currentChannel || !channelId || !newTaskTitle.trim()) return;
    setCreating(true);
    try {
      await createTasks(channelId, [newTaskTitle.trim()]);
      setNewTaskTitle("");
      setError(null);
      refresh();
    } catch (e) {
      console.error(e);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  }

  if (!currentChannel || !channelId) {
    return (
      <div className="tasks-panel">
        <div className="tasks-empty">Select a channel to view tasks.</div>
      </div>
    );
  }

  return (
    <div className="tasks-panel">
      <div className="tasks-panel-header">
        <div className="tasks-panel-header-copy">
          <span className="tasks-panel-kicker">[board::channel]</span>
          <span className="tasks-panel-title">Tasks</span>
        </div>
        <span className="tasks-panel-channel">#{currentChannel.name}</span>
      </div>
      {error && <FormError>{error}</FormError>}

      {loading && tasks.length === 0 ? (
        <div className="tasks-empty">Loading tasks...</div>
      ) : (
        <div className="tasks-board">
          {COLUMNS.map(({ status, label }) => {
            const col = tasks.filter((t) => t.status === status);
            return (
              <div key={status} className="task-column" data-status={status}>
                <div className="task-column-header">
                  {label}
                  <span className="task-count-badge">{col.length}</span>
                </div>
                {col.map((task) => (
                  <TaskCard
                    key={task.taskNumber}
                    task={task}
                    parentChannelId={channelId}
                    parentSlug={currentChannel.name}
                  />
                ))}
                {status === "todo" && (
                  <div className="new-task-row">
                    <input
                      className="new-task-input"
                      placeholder="New task title..."
                      value={newTaskTitle}
                      onChange={(e) => setNewTaskTitle(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && handleCreate()}
                    />
                    <button
                      className="new-task-submit"
                      onClick={handleCreate}
                      disabled={creating || !newTaskTitle.trim()}
                    >
                      +
                    </button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
