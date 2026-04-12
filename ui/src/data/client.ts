const BASE = ''

export type ApiErrorCode =
  | 'INTERNAL_ERROR'
  | 'AGENT_NAME_TAKEN'
  | 'CHANNEL_NAME_TAKEN'
  | 'TEAM_NAME_TAKEN'
  | 'AGENT_RESTART_FAILED'
  | 'AGENT_DELETE_WORKSPACE_CLEANUP_FAILED'
  | 'CHANNEL_OPERATION_UNSUPPORTED'
  | 'MESSAGE_NOT_A_MEMBER'

export class ApiError extends Error {
  readonly status: number
  readonly code?: ApiErrorCode

  constructor(status: number, message: string, code?: ApiErrorCode) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

async function parseResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    const msg = (body as { error?: string }).error ?? res.statusText
    const code = (body as { code?: ApiErrorCode }).code
    throw new ApiError(res.status, msg, code)
  }
  return res.json() as Promise<T>
}

export async function get<T>(path: string, init?: RequestInit): Promise<T> {
  return parseResponse<T>(await fetch(`${BASE}${path}`, init))
}

export async function post<T>(path: string, body?: unknown, init?: RequestInit): Promise<T> {
  return parseResponse<T>(
    await fetch(`${BASE}${path}`, {
      method: 'POST',
      headers: body ? { 'Content-Type': 'application/json' } : undefined,
      body: body ? JSON.stringify(body) : undefined,
      ...init,
    })
  )
}

export async function patch<T>(path: string, body: unknown): Promise<T> {
  return parseResponse<T>(
    await fetch(`${BASE}${path}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  )
}

export async function del<T>(path: string): Promise<T> {
  return parseResponse<T>(await fetch(`${BASE}${path}`, { method: 'DELETE' }))
}

export function resourceUrl(path: string): string {
  return `${BASE}${path}`
}
