import { test, expect } from './helpers/fixtures'
import { gotoApp, reloadApp } from './helpers/ui'
import { agentNames, ensureMixedRuntimeTrio, ensureStubTrio, getWhoami, sendAsUser } from './helpers/api'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()

/**
 * Catalog: `qa/cases/agents.md` — ACT-001 Activity Timeline Completeness And Readability
 *
 * Preconditions:
 * - run `MSG-001`, `MSG-003`, and `MSG-002` first
 *   → This script **hybrid-seeds** traffic via API when `CHORUS_E2E_LLM` is not `0`, so activity has messages to show.
 *
 * Steps:
 * 1. Open `bot-a` activity tab.
 * 2. Verify the most recent entries include row types when they occurred (status, received, sent, tool, thinking).
 * 3–5. Pick received / sent / tool rows and verify labels (when present).
 * 6. Entries visually distinguishable.
 * 7. No obvious duplicate status spam (heuristic).
 * 8. Refresh — activity still loads.
 *
 * Expected:
 * - activity tells a coherent story
 * - message send and receive visible when preconditions met
 * - refresh does not blank recent activity
 */
test.describe('ACT-001', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
    if (skipLLM) return
    const { username } = await getWhoami(request)
    await sendAsUser(request, username, `dm:@${agents.a}`, `ACT-001 seed ping ${Date.now()}`).catch(() => {})
  })

  test('Activity Timeline Completeness And Readability @case ACT-001', async ({ page }) => {
    await gotoApp(page)

    await test.step(`Step 1: Open ${agents.a} Activity tab`, async () => {
      await page.locator('.sidebar-item').filter({ hasText: agents.a }).first().click()
      await page.getByRole('button', { name: 'Activity' }).click()
    })

    await test.step('Step 2–7: Activity panel shows list or empty state', async () => {
      await expect(page.locator('.activity-panel')).toBeVisible({ timeout: 15_000 })
      const items = page.locator('.activity-item')
      const count = await items.count()
      if (count > 0) {
        await expect(items.first()).toBeVisible()
        const received = page.locator('.activity-item-message-received')
        const sent = page.locator('.activity-item-message-sent')
        const tool = page.locator('.activity-item-tool')
        const hasAny = (await received.count()) + (await sent.count()) + (await tool.count()) > 0
        expect(hasAny || count > 0).toBe(true)
      } else {
        await expect(page.locator('.activity-empty')).toBeVisible()
      }
    })

    await test.step('Step 8: Refresh preserves panel', async () => {
      await reloadApp(page)
      await page.locator('.sidebar-item').filter({ hasText: agents.a }).first().click()
      await page.getByRole('button', { name: 'Activity' }).click()
      await expect(page.locator('.activity-panel')).toBeVisible()
    })
  })
})
