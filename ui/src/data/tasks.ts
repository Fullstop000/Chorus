import { get, post } from './client'
import { queryString } from './common'
import type {
  CreateTasksRequest,
  ClaimTasksRequest,
  UnclaimTaskRequest,
  UpdateTaskStatusRequest,
} from './requests'

// ── Types (source of truth) ──

export type TaskStatus = 'todo' | 'in_progress' | 'in_review' | 'done'

export interface TaskInfo {
  id?: string
  taskNumber: number
  title: string
  status: TaskStatus
  channelId?: string
  claimedByName?: string
  createdByName?: string
  createdAt?: string
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

export function groupTasksByStatus(tasks: TaskInfo[]): Record<TaskStatus, TaskInfo[]> {
  const result: Record<string, TaskInfo[]> = { todo: [], in_progress: [], in_review: [], done: [] }
  for (const task of tasks) {
    result[task.status].push(task)
  }
  return result as Record<TaskStatus, TaskInfo[]>
}
