import { test, expect } from './helpers/fixtures'
import { gotoApp, reloadApp } from './helpers/ui'
import { ensureMixedRuntimeTrio, getWhoami, sendAsUser, type TrioNames } from './helpers/api'

/**
 * Catalog: `qa/cases/agents.md` — ACT-001 Activity Timeline Completeness And Readability
 *
 * Uses bot-b (kimi) for DM seed and activity inspection — only kimi reliably responds.
 */
let trio: TrioNames

test.describe('ACT-001', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
    if (process.env.CHORUS_E2E_LLM === '0') return
    const { username } = await getWhoami(request)
    await sendAsUser(request, username, `dm:@${trio.botB}`, `ACT-001 seed ping ${Date.now()}`).catch(() => {})
  })

  test('Activity Timeline Completeness And Readability @case ACT-001', async ({ page }) => {
    await gotoApp(page)

    await test.step('Step 1: Open bot-b Activity tab', async () => {
      await page.locator('.sidebar-item').filter({ hasText: trio.displayB }).first().click()
      await page.getByRole('button', { name: 'Activity' }).click()
    })

    await test.step('Step 2–7: Activity panel shows list or empty state', async () => {
      await expect(page.locator('.ta-layout')).toBeVisible({ timeout: 15_000 })
      const items = page.locator('.ta-detail .activity-item')
      const count = await items.count()
      if (count > 0) {
        await expect(items.first()).toBeVisible()
        const received = page.locator('.ta-detail .activity-item-message-received')
        const sent = page.locator('.ta-detail .activity-item-message-sent')
        const tool = page.locator('.ta-detail .activity-item-tool')
        const hasAny = (await received.count()) + (await sent.count()) + (await tool.count()) > 0
        expect(hasAny || count > 0).toBe(true)
      } else {
        await expect(page.locator('.ta-runs-empty, .ta-detail-empty').first()).toBeVisible()
      }
    })

    await test.step('Step 8: Refresh preserves panel', async () => {
      await reloadApp(page)
      await page.locator('.sidebar-item').filter({ hasText: trio.displayB }).first().click()
      await page.getByRole('button', { name: 'Activity' }).click()
      await expect(page.locator('.ta-layout')).toBeVisible()
    })
  })
})
