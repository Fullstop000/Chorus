import { get, post } from './client'
import { queryString } from './common'
import type { TaskStatus, TasksResponse, TaskInfo } from '../types'

export type { TaskStatus, TaskInfo, TasksResponse } from '../types'

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
  return post(conversationPath(conversationId, '/tasks'), {
    tasks: titles.map((title) => ({ title })),
  })
}

export function claimTasks(
  conversationId: string,
  taskNumbers: number[]
): Promise<{ results: Array<{ taskNumber: number; success: boolean; reason?: string }> }> {
  return post(conversationPath(conversationId, '/tasks/claim'), {
    task_numbers: taskNumbers,
  })
}

export function unclaimTask(conversationId: string, taskNumber: number): Promise<void> {
  return post(conversationPath(conversationId, '/tasks/unclaim'), {
    task_number: taskNumber,
  })
}

export function updateTaskStatus(
  conversationId: string,
  taskNumber: number,
  status: TaskStatus
): Promise<void> {
  return post(conversationPath(conversationId, '/tasks/update-status'), {
    task_number: taskNumber,
    status,
  })
}

export function groupTasksByStatus(tasks: TaskInfo[]): Record<TaskStatus, TaskInfo[]> {
  const result: Record<string, TaskInfo[]> = { todo: [], in_progress: [], in_review: [], done: [] }
  for (const task of tasks) {
    result[task.status].push(task)
  }
  return result as Record<TaskStatus, TaskInfo[]>
}
