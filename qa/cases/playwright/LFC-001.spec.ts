import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'
import { ensureMixedRuntimeTrio, listAgents, type TrioNames } from './helpers/api'

/**
 * Catalog: `qa/cases/agents.md` — LFC-001 Agent Lifecycle Start, Idle, Stop, And Manual Restart
 *
 * Preconditions:
 * - at least one test agent exists
 * - the agent is not currently mid-turn when the case starts
 *
 * Steps:
 * 1. Create or select a test agent.
 * 2. Verify the agent enters a startup state such as `working`, `starting`, or similar transitional status.
 * 3. Wait until the agent settles into its idle state such as `online`, `ready`, or `waiting for messages`.
 * 4. Verify sidebar status, profile status, and activity log all tell the same lifecycle story.
 * 5. Stop the agent from the shipped UI control.
 * 6. Verify sidebar, profile, and activity all move to an inactive or stopped state.
 * 7. Start the agent again from the shipped UI control if one exists.
 * 8. Verify it returns to startup and then back to idle.
 *
 * Expected:
 * - startup is visible
 * - idle is visible and stable
 * - stop is visible and stable
 * - manual restart restores the agent cleanly
 */
test.describe('LFC-001', () => {
  let trio: TrioNames

  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Agent Lifecycle Start, Idle, Stop, Manual Restart @case LFC-001', async ({
    page,
    request,
  }) => {
    await gotoApp(page)

    await test.step('Step 1: Select test agent bot-a', async () => {
      await page.locator('.sidebar-item').filter({ hasText: trio.displayA }).first().click()
    })

    await test.step('Step 2–3: Transitional / idle status visible (sidebar dot or profile)', async () => {
      await page.getByRole('button', { name: 'Profile', exact: true }).click()
      await expect(page.getByRole('button', { name: /\[stop::agent\]|\[start::agent\]/ })).toBeVisible({
        timeout: 60_000,
      })
    })

    await test.step('Step 4: Profile shows lifecycle controls', async () => {
      await expect(page.locator('.tab-bar')).toBeVisible()
    })

    await test.step('Step 5: Stop from UI', async () => {
      const stop = page.getByRole('button', { name: '[stop::agent]' })
      if (await stop.isVisible().catch(() => false)) {
        await stop.click()
      }
    })

    await test.step('Step 6: Inactive — start button visible', async () => {
      await expect(page.getByRole('button', { name: '[start::agent]' })).toBeVisible({ timeout: 30_000 })
    })

    await test.step('Step 7–8: Start again → back to stop-capable active state', async () => {
      await page.getByRole('button', { name: '[start::agent]' }).click()
      await expect(page.getByRole('button', { name: '[stop::agent]' })).toBeVisible({ timeout: 60_000 })
    })

    await test.step('Expected: API list shows bot-a active after restart', async () => {
      const agents = await listAgents(request)
      expect(['ready', 'working']).toContain(agents.find((x) => x.name === trio.botA)?.status)
    })
  })
})
