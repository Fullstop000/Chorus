import { useState } from 'react'
import { useApp } from '../store'
import { useTasks } from '../hooks/useTasks'
import { createTasks, updateTaskStatus, claimTasks } from '../api'
import type { TaskInfo, TaskStatus } from '../types'
import './TasksPanel.css'

const COLUMNS: { status: TaskStatus; label: string }[] = [
  { status: 'todo', label: 'To Do' },
  { status: 'in_progress', label: 'In Progress' },
  { status: 'in_review', label: 'In Review' },
  { status: 'done', label: 'Done' },
]

function TaskCard({
  task,
  currentUser,
  channel,
  onRefresh,
  onError,
}: {
  task: TaskInfo
  currentUser: string
  channel: string
  onRefresh: () => void
  onError: (message: string | null) => void
}) {
  const nextStatus: Record<TaskStatus, TaskStatus | null> = {
    todo: 'in_progress',
    in_progress: 'in_review',
    in_review: 'done',
    done: null,
  }
  const next = nextStatus[task.status]
  const canAdvance =
    !!next &&
    (
      (!task.claimedByName && task.status === 'todo') ||
      task.claimedByName === currentUser
    )

  async function advance() {
    if (!next || !canAdvance) return
    try {
      // Auto-claim when starting a task (backend requires claim before status update)
      if (task.status === 'todo') {
        await claimTasks(currentUser, channel, [task.taskNumber])
        onError(null)
        onRefresh()
        return
      }
      await updateTaskStatus(currentUser, channel, task.taskNumber, next)
      onError(null)
      onRefresh()
    } catch (e) {
      console.error(e)
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div
      className="task-card"
      onClick={advance}
      title={
        !next
          ? 'Done'
          : canAdvance
          ? `Advance to ${next}`
          : task.claimedByName
          ? `Claimed by ${task.claimedByName}`
          : 'Unavailable'
      }
      style={{ cursor: canAdvance ? 'pointer' : 'not-allowed', opacity: canAdvance ? 1 : 0.7 }}
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
  )
}

export function TasksPanel() {
  const { currentUser, selectedChannel } = useApp()
  const { tasks, loading, refresh } = useTasks(currentUser, selectedChannel)
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleCreate() {
    if (!selectedChannel || !newTaskTitle.trim()) return
    setCreating(true)
    try {
      await createTasks(currentUser, selectedChannel, [newTaskTitle.trim()])
      setNewTaskTitle('')
      setError(null)
      refresh()
    } catch (e) {
      console.error(e)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setCreating(false)
    }
  }

  if (!selectedChannel) {
    return (
      <div className="tasks-panel">
        <div className="tasks-empty">Select a channel to view tasks.</div>
      </div>
    )
  }

  return (
    <div className="tasks-panel">
      <div className="tasks-panel-header">
        <span className="tasks-panel-title">Tasks — {selectedChannel}</span>
      </div>
      {error && <div className="error-banner">{error}</div>}

      {loading && tasks.length === 0 ? (
        <div className="tasks-empty">Loading tasks...</div>
      ) : (
        <div className="tasks-board">
          {COLUMNS.map(({ status, label }) => {
            const col = tasks.filter((t) => t.status === status)
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
                    currentUser={currentUser}
                    channel={selectedChannel}
                    onRefresh={refresh}
                    onError={setError}
                  />
                ))}
                {status === 'todo' && (
                  <div className="new-task-row">
                    <input
                      className="new-task-input"
                      placeholder="New task title..."
                      value={newTaskTitle}
                      onChange={(e) => setNewTaskTitle(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
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
            )
          })}
        </div>
      )}
    </div>
  )
}
