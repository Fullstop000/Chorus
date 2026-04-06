const BASE = ''

async function parseResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error((err as { error?: string }).error ?? res.statusText)
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
