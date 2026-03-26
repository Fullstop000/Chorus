import { test, expect } from '@playwright/test'
import { listAgents } from './helpers/api'
import { createAgentViaUi } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — AGT-001 Create Three Agents And Verify Sidebar Presence
 *
 * Preconditions:
 * - no existing test agents in the fresh data dir (or all three bot-a/b/c already present — see beforeAll)
 *
 * Steps:
 * 1. Create `bot-a`.
 * 2. Create `bot-b`.
 * 3. Create `bot-c`.
 * 4. Verify each agent appears in the sidebar.
 * 5. Click each agent once and verify its tabs load without crashing.
 *
 * Expected:
 * - all three agents are created successfully
 * - sidebar updates after each creation
 * - each agent is selectable
 */
test.describe('AGT-001', () => {
  test.beforeAll(async ({ request }) => {
    const agents = await listAgents(request)
    const need = ['bot-a', 'bot-b', 'bot-c'].filter((n) => !agents.some((a) => a.name === n))
    if (need.length > 0 && need.length < 3) {
      throw new Error(
        `AGT-001: need none or all of bot-a/b/c; missing ${need.join(', ')}. Use a fresh --data-dir.`
      )
    }
  })

  test('Create Three Agents And Verify Sidebar Presence @case AGT-001', async ({
    page,
    request,
  }) => {
    const before = await listAgents(request)
    const already = ['bot-a', 'bot-b', 'bot-c'].every((n) => before.some((a) => a.name === n))

    await page.goto('/', { waitUntil: 'networkidle' })

    if (!already) {
      await test.step('Step 1: Create bot-a', async () => {
        await createAgentViaUi(page, { name: 'bot-a', runtime: 'claude', model: 'sonnet' })
      })
      await test.step('Step 2: Create bot-b', async () => {
        await createAgentViaUi(page, { name: 'bot-b', runtime: 'claude', model: 'opus' })
      })
      await test.step('Step 3: Create bot-c', async () => {
        await createAgentViaUi(page, { name: 'bot-c', runtime: 'codex', model: 'gpt-5.4-mini' })
      })
    }

    await test.step('Step 4: Each agent appears in the sidebar', async () => {
      for (const name of ['bot-a', 'bot-b', 'bot-c']) {
        await expect(page.locator('.sidebar-item').filter({ hasText: name }).first()).toBeVisible()
      }
    })

    await test.step('Step 5: Click each agent — tabs load', async () => {
      for (const name of ['bot-a', 'bot-b', 'bot-c']) {
        await page.locator('.sidebar-item').filter({ hasText: name }).first().click()
        await expect(page.locator('.tab-bar')).toBeVisible()
      }
    })
  })
})
