const BASE = ''

export type ApiErrorCode =
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
  return parseResponse<T>(
    await fetch(`${BASE}${path}`, {
      cache: 'no-store',
      ...init,
    })
  )
}

export async function post<T>(path: string, body?: unknown, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers)
  let requestBody: BodyInit | undefined = init?.body ?? undefined

  if (body instanceof FormData) {
    requestBody = body
  } else if (body !== undefined) {
    headers.set('Content-Type', 'application/json')
    requestBody = JSON.stringify(body)
  }

  return parseResponse<T>(
    await fetch(`${BASE}${path}`, {
      method: 'POST',
      ...init,
      headers: Array.from(headers.keys()).length > 0 ? headers : undefined,
      body: requestBody,
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
