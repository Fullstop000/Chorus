import { useTasksStore } from '../store/tasksStore'
import type { TaskInfo } from '../data/tasks'

/**
 * Read a single task from the in-memory tasks slice. Returns `null` when the
 * task isn't loaded yet — callers (e.g. `TaskCardContainer`) render nothing
 * in that gap rather than blocking with a spinner; the row resolves on the
 * next list fetch or `task_update` frame.
 */
export function useTask(taskId: string): TaskInfo | null {
  return useTasksStore((s) => s.tasksById[taskId] ?? null)
}
