import { test, expect } from './helpers/fixtures'
import { listAgents } from './helpers/api'
import { createAgentViaUi, gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — AGT-001 Create Agent And Verify Sidebar Presence
 *
 * Preconditions:
 * - fresh data dir
 *
 * Steps:
 * 1. Create `smoke-bot` using the Codex runtime.
 * 2. Verify the agent appears in the sidebar.
 * 3. Click the agent and verify its tabs load without crashing.
 *
 * Expected:
 * - agent is created successfully
 * - sidebar updates after creation
 * - agent is selectable and tabs render
 */
test.describe('AGT-001', () => {
  test('Create Agent And Verify Sidebar Presence @case AGT-001', async ({ page, request }) => {
    const before = await listAgents(request)
    const already = before.some(
      (a) => a.display_name === 'smoke-bot' || a.name === 'smoke-bot' || a.name.startsWith('smoke-bot-')
    )

    await gotoApp(page)

    if (!already) {
      await test.step('Step 1: Create smoke-bot (codex)', async () => {
        await createAgentViaUi(page, { name: 'smoke-bot', runtime: 'codex', model: 'gpt-5.4-mini' })
      })
    }

    await test.step('Step 2: smoke-bot appears in the sidebar', async () => {
      await expect(page.locator('.sidebar-item').filter({ hasText: 'smoke-bot' }).first()).toBeVisible()
    })

    await test.step('Step 3: Click agent — tabs load', async () => {
      await page.locator('.sidebar-item').filter({ hasText: 'smoke-bot' }).first().click()
      await expect(page.locator('.tab-bar')).toBeVisible()
    })
  })
})
