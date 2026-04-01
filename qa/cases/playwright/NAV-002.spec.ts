import { test, expect } from './helpers/fixtures'
import { waitForAppReady } from './helpers/ui'

/**
 * Idle shell should not poll sidebar resources.
 */
test.describe('NAV-002', () => {
  test('App shell does not poll sidebar resources while idle @case NAV-002', async ({
    page,
  }) => {
    const counts = {
      humans: 0,
      channels: 0,
      agents: 0,
      teams: 0,
    }

    page.on('request', (request) => {
      const url = new URL(request.url())
      if (url.pathname === '/api/humans') counts.humans += 1
      if (url.pathname === '/api/channels') counts.channels += 1
      if (url.pathname === '/api/agents') counts.agents += 1
      if (url.pathname === '/api/teams') counts.teams += 1
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    // Wait for the app to fully initialise so all startup requests are counted,
    // then observe for 2 s to confirm no polling kicks in.
    await waitForAppReady(page)
    await page.waitForTimeout(2_000)

    expect(counts.humans).toBe(1)
    expect(counts.channels).toBe(1)
    expect(counts.agents).toBe(1)
    expect(counts.teams).toBe(1)
  })
})
