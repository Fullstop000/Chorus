import { useState } from "react";
import { User } from "lucide-react";
import { useStore } from "../../store";
import { useTasks } from "../../hooks/useTasks";
import { createTasks } from "../../data";
import type { TaskInfo, TaskStatus } from "./types";
import { FormError } from "@/components/ui/form";
import "./TasksPanel.css";

/**
 * Kanban columns. The unified lifecycle adds `proposed` and `dismissed`, but
 * those live on the parent-channel TaskCard (one for each proposal pending a
 * decision) — the kanban only shows committed work.
 */
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
  const activeTab = useStore((s) => s.activeTab);

  // Primary click opens the task detail view. Status advancement (previously
  // auto-advance on card click) moves to affordances inside the detail page
  // in Task 9 — clicking a row is now pure navigation.
  //
  // `returnToTab` snapshots the tab the user was on so the back button / Esc
  // can restore it instead of always dropping users on Tasks.
  function openDetail() {
    setCurrentTaskDetail({
      parentChannelId,
      parentSlug,
      taskNumber: task.taskNumber,
      returnToTab: activeTab,
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
        {task.owner && (
          <span className="task-card-claimed">
            <User size={11} aria-hidden="true" />
            {task.owner}
          </span>
        )}
        {!task.owner && task.createdBy && (
          <span>by {task.createdBy}</span>
        )}
      </span>
    </button>
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
