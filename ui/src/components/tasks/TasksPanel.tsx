import { useState } from "react";
import { useStore } from "../../store";
import { useTasks } from "../../hooks/useTasks";
import { createTasks } from "../../data";
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
  parentChannelId,
  parentSlug,
}: {
  task: TaskInfo;
  parentChannelId: string;
  parentSlug: string;
}) {
  const setCurrentTaskDetail = useStore((s) => s.setCurrentTaskDetail);

  // Primary click opens the task detail view. Status advancement (previously
  // auto-advance on card click) moves to affordances inside the detail page
  // in Task 9 — clicking a row is now pure navigation.
  function openDetail() {
    setCurrentTaskDetail({
      parentChannelId,
      parentSlug,
      taskNumber: task.taskNumber,
    });
  }

  return (
    <div
      className="task-card"
      onClick={openDetail}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          openDetail();
        }
      }}
      title={`Open task #${task.taskNumber}`}
      style={{ cursor: "pointer" }}
    >
      <div className="task-card-number">#{task.taskNumber}</div>
      <div className="task-card-title">{task.title}</div>
      <div className="task-card-meta">
        {task.claimedByName && (
          <span className="task-card-claimed">⚡ {task.claimedByName}</span>
        )}
        {!task.claimedByName && task.createdByName && (
          <span>by {task.createdByName}</span>
        )}
      </div>
    </div>
  );
}

export function TasksPanel() {
  const { currentUser, currentChannel } = useStore();
  const channelId = currentChannel?.id ?? null;
  const { tasks, loading, refresh } = useTasks(currentUser, channelId);
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
