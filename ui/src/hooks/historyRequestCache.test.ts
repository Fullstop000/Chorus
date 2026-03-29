import { afterEach, describe, expect, it, vi } from 'vitest'

import { loadSharedRequest, resetSharedRequests } from './historyRequestCache'

describe('history request cache', () => {
  afterEach(() => {
    resetSharedRequests()
    vi.useRealTimers()
  })

  it('shares concurrent requests for the same key', async () => {
    let calls = 0
    const loader = vi.fn(async () => {
      calls += 1
      await Promise.resolve()
      return { calls }
    })

    const [first, second] = await Promise.all([
      loadSharedRequest('history:alice:#all', loader),
      loadSharedRequest('history:alice:#all', loader),
    ])

    expect(loader).toHaveBeenCalledTimes(1)
    expect(first).toEqual({ calls: 1 })
    expect(second).toEqual({ calls: 1 })
  })

  it('reuses a just-finished result briefly to absorb strict-mode remounts', async () => {
    vi.useFakeTimers()
    let calls = 0
    const loader = vi.fn(async () => {
      calls += 1
      return { calls }
    })

    const first = await loadSharedRequest('history:alice:#all', loader)
    const second = await loadSharedRequest('history:alice:#all', loader)

    expect(loader).toHaveBeenCalledTimes(1)
    expect(first).toEqual({ calls: 1 })
    expect(second).toEqual({ calls: 1 })

    vi.advanceTimersByTime(251)

    await expect(loadSharedRequest('history:alice:#all', loader)).resolves.toEqual({ calls: 2 })
    expect(loader).toHaveBeenCalledTimes(2)
  })
})
