import { get, post } from './client'
import { queryString } from './common'
import type {
  CreateTasksRequest,
  ClaimTasksRequest,
  UnclaimTaskRequest,
  UpdateTaskStatusRequest,
} from './requests'

// ── Types (source of truth) ──

/**
 * Unified task lifecycle. Pre-acceptance (`proposed`/`dismissed`) sit beside
 * the four post-acceptance kanban states. Forward-only — no reverse edges.
 * Wire form mirrors the Rust `TaskStatus::as_str()` enum.
 */
export type TaskStatus =
  | 'proposed'
  | 'dismissed'
  | 'todo'
  | 'in_progress'
  | 'in_review'
  | 'done'

/**
 * Task wire shape. Field names match the camelCase Rust serialization
 * (`#[serde(rename_all = "camelCase")]` on `TaskInfo`). The TaskCard host
 * message in the parent channel carries this same payload — keep the two in
 * sync.
 */
export interface TaskInfo {
  /** UUID primary key — store keying. */
  id: string
  /** Per-channel task number. Stable handle for MCP/CLI. */
  taskNumber: number
  title: string
  status: TaskStatus
  /** Current claimer handle. `null` when unclaimed or pre-acceptance. */
  owner: string | null
  /** Creator handle. */
  createdBy: string
  /** Insert time (ISO8601). */
  createdAt: string
  /** Last mutation time (ISO8601). */
  updatedAt: string
  /** Child sub-channel id; `null` for legacy/proposed rows. */
  subChannelId: string | null
  /** Child sub-channel name for deep-linking. */
  subChannelName: string | null
  /** Source message id this task was carved from (proposal flow). */
  sourceMessageId?: string | null
  /** Snapshot of the source message's sender display name. */
  snapshotSenderName?: string | null
  /** Snapshot of the source message's sender type (`human`/`agent`/`system`). */
  snapshotSenderType?: string | null
  /** Snapshot of the source message's content at carve time. */
  snapshotContent?: string | null
  /** Snapshot of the source message's created_at timestamp. */
  snapshotCreatedAt?: string | null
}

export interface TasksResponse {
  tasks: TaskInfo[]
}

// ── API functions ──

function conversationPath(conversationId: string, suffix = ''): string {
  return `/api/conversations/${encodeURIComponent(conversationId)}${suffix}`
}

export function getTasks(
  conversationId: string,
  status: 'all' | TaskStatus = 'all'
): Promise<TasksResponse> {
  return get(`${conversationPath(conversationId, '/tasks')}${queryString({ status })}`)
}

export function getTaskDetail(
  conversationId: string,
  taskNumber: number,
): Promise<TaskInfo> {
  return get(conversationPath(conversationId, `/tasks/${taskNumber}`))
}

export function createTasks(conversationId: string, titles: string[]): Promise<TasksResponse> {
  const payload: CreateTasksRequest = { tasks: titles.map((title) => ({ title })) }
  return post(conversationPath(conversationId, '/tasks'), payload)
}

export function claimTasks(
  conversationId: string,
  taskNumbers: number[]
): Promise<{ results: Array<{ taskNumber: number; success: boolean; reason?: string }> }> {
  const payload: ClaimTasksRequest = { task_numbers: taskNumbers }
  return post(conversationPath(conversationId, '/tasks/claim'), payload)
}

export function unclaimTask(conversationId: string, taskNumber: number): Promise<void> {
  const payload: UnclaimTaskRequest = { task_number: taskNumber }
  return post(conversationPath(conversationId, '/tasks/unclaim'), payload)
}

export function updateTaskStatus(
  conversationId: string,
  taskNumber: number,
  status: TaskStatus
): Promise<void> {
  const payload: UpdateTaskStatusRequest = { task_number: taskNumber, status }
  return post(conversationPath(conversationId, '/tasks/update-status'), payload)
}

// ── Transforms ──

/**
 * Bucket tasks by status. Initialises every key in the literal so the kanban
 * filter can iterate columns without `?? []` plumbing — even pre-acceptance
 * rows (`proposed`/`dismissed`) get an empty bucket. Consumers that only
 * render committed work filter by the four kanban statuses themselves.
 */
export function groupTasksByStatus(tasks: TaskInfo[]): Record<TaskStatus, TaskInfo[]> {
  const result: Record<TaskStatus, TaskInfo[]> = {
    proposed: [],
    dismissed: [],
    todo: [],
    in_progress: [],
    in_review: [],
    done: [],
  }
  for (const task of tasks) {
    result[task.status].push(task)
  }
  return result
}
