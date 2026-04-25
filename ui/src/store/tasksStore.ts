import { create } from 'zustand'
import type { TaskInfo, TaskStatus } from '../data/tasks'

/**
 * Patch shape applied when a `task_update` frame arrives. Mirrors the realtime
 * `TaskUpdateEvent` payload subset that the kanban + parent-channel TaskCard
 * actually need to repaint — the frame carries the rest, but it's not stored
 * here yet. If a row isn't already in `tasksById`, the patch is a no-op:
 * cards/lists fetch the full row before adding it.
 */
export interface TaskUpdate {
  taskId: string
  status: TaskStatus
  owner: string | null
  subChannelId: string | null
  updatedAt: string
}

interface TasksState {
  /** Full task rows keyed by id. */
  tasksById: Record<string, TaskInfo>
  /** Replace the whole map — used after a list fetch. */
  setAll: (tasks: TaskInfo[]) => void
  /** Insert or replace a single row — used after a single fetch. */
  upsert: (task: TaskInfo) => void
  /**
   * Apply a realtime delta. Drops silently when the row is unknown; callers
   * that care about freshly-created tasks must fetch first or handle the
   * `task_card` host-message branch.
   */
  applyUpdate: (update: TaskUpdate) => void
}

export const useTasksStore = create<TasksState>((set) => ({
  tasksById: {},
  setAll: (tasks) =>
    set({
      tasksById: Object.fromEntries(tasks.map((t) => [t.id, t])),
    }),
  upsert: (task) =>
    set((state) => ({
      tasksById: { ...state.tasksById, [task.id]: task },
    })),
  applyUpdate: (update) =>
    set((state) => {
      const prev = state.tasksById[update.taskId]
      // Unknown row: don't fabricate a partial entry from the delta — the
      // realtime patch lacks title / createdBy / createdAt etc., and a
      // half-formed row would crash any consumer that expects a complete
      // TaskInfo. Wait for a fetch to populate the row first.
      if (!prev) return state
      return {
        tasksById: {
          ...state.tasksById,
          [update.taskId]: {
            ...prev,
            status: update.status,
            owner: update.owner,
            subChannelId: update.subChannelId,
            updatedAt: update.updatedAt,
          },
        },
      }
    }),
}))
