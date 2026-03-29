import { test, expect } from '@playwright/test'

/**
 * Idle shell should not poll sidebar resources.
 */
test.describe('NAV-002', () => {
  test('App shell does not poll sidebar resources while idle @case NAV-002', async ({
    page,
  }) => {
    const counts = {
      serverInfo: 0,
      channels: 0,
      agents: 0,
      teams: 0,
    }

    page.on('request', (request) => {
      const url = new URL(request.url())
      if (url.pathname === '/api/server-info') counts.serverInfo += 1
      if (url.pathname === '/api/channels') counts.channels += 1
      if (url.pathname === '/api/agents') counts.agents += 1
      if (url.pathname === '/api/teams') counts.teams += 1
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.waitForTimeout(6_500)

    expect(counts.serverInfo).toBe(1)
    expect(counts.channels).toBe(1)
    expect(counts.agents).toBe(1)
    expect(counts.teams).toBe(1)
  })
})
