import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'
import { ensureMixedRuntimeTrio, listAgents } from './helpers/api'

/**
 * Catalog: `qa/cases/agents.md` — PRF-001 Agent Profile Accuracy During Lifecycle Changes
 *
 * Preconditions:
 * - at least one active agent exists
 *
 * Steps:
 * 1. Open `bot-a` profile.
 * 2. Record visible status and activity.
 * 3. Stop the agent from the UI.
 * 4. Verify the profile updates to inactive or stopped state.
 * 5. Start or wake the agent again if supported.
 * 6. Verify the profile updates back to active state.
 *
 * Expected:
 * - profile status changes promptly and correctly
 * - action buttons match the actual lifecycle state
 * - no stale active label after stop
 */
test.describe('PRF-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Agent Profile Accuracy During Lifecycle Changes @case PRF-001', async ({ page, request }) => {
    await gotoApp(page)

    await test.step('Step 1: Open bot-a profile', async () => {
      await page.locator('.sidebar-item').filter({ hasText: 'bot-a' }).first().click()
      await page.getByRole('button', { name: 'Profile' }).click()
    })

    await test.step('Step 2: Visible status / activity (profile panel loaded)', async () => {
      await expect(page.getByRole('button', { name: /\[stop::agent\]|\[start::agent\]/ })).toBeVisible({
        timeout: 60_000,
      })
    })

    await test.step('Step 3: Stop from UI', async () => {
      const stop = page.getByRole('button', { name: '[stop::agent]' })
      await expect(stop).toBeVisible()
      await stop.click()
    })

    await test.step('Step 4: Profile reflects inactive (start shown)', async () => {
      await expect(page.getByRole('button', { name: '[start::agent]' })).toBeVisible({ timeout: 30_000 })
    })

    await test.step('Step 5–6: Start again → active (stop shown)', async () => {
      await page.getByRole('button', { name: '[start::agent]' }).click()
      await expect(page.getByRole('button', { name: '[stop::agent]' })).toBeVisible({ timeout: 60_000 })
    })

    await test.step('Expected: list API bot-a active', async () => {
      const agents = await listAgents(request)
      expect(agents.find((x) => x.name === 'bot-a')?.status).toBe('active')
    })
  })
})
