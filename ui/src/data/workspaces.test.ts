import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  createWorkspace,
  deleteWorkspace,
  listWorkspaces,
  switchWorkspace,
} from './workspaces'

function jsonResponse(body: unknown, init: ResponseInit = {}) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
    ...init,
  })
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('workspace api client', () => {
  it('lists workspaces from /api/workspaces', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse([]))
    vi.stubGlobal('fetch', fetchMock)

    await expect(listWorkspaces()).resolves.toEqual([])
    expect(fetchMock).toHaveBeenCalledWith('/api/workspaces', {
      cache: 'no-store',
    })
  })

  it('creates and switches workspaces through server api requests', async () => {
    const workspace = {
      id: 'ws-1',
      name: 'Alpha',
      slug: 'alpha',
      mode: 'local_only',
      created_by_human: 'alice',
      created_at: '2026-04-25T00:00:00Z',
      active: true,
    }
    const fetchMock = vi.fn().mockImplementation(() => Promise.resolve(jsonResponse(workspace)))
    vi.stubGlobal('fetch', fetchMock)

    await expect(createWorkspace('Alpha')).resolves.toMatchObject({ slug: 'alpha' })
    await expect(switchWorkspace('ws-1')).resolves.toMatchObject({ active: true })

    expect(fetchMock).toHaveBeenNthCalledWith(1, '/api/workspaces', {
      method: 'POST',
      headers: new Headers({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({ name: 'Alpha' }),
    })
    expect(fetchMock).toHaveBeenNthCalledWith(2, '/api/workspaces/switch', {
      method: 'POST',
      headers: new Headers({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({ workspace: 'ws-1' }),
    })
  })

  it('deletes a workspace by encoded selector', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        deleted_id: 'ws/one',
        active_workspace: null,
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await expect(deleteWorkspace('ws/one')).resolves.toMatchObject({
      deleted_id: 'ws/one',
    })
    expect(fetchMock).toHaveBeenCalledWith('/api/workspaces/ws%2Fone', {
      method: 'DELETE',
    })
  })
})
