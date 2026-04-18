import { test, expect } from './helpers/fixtures'
import { createAgentViaUi, openAgentTab, gotoApp } from './helpers/ui'
import { createAgentApi, getAgentDetail, listAgents } from './helpers/api'

/**
 * Catalog: `qa/cases/agents.md` — AGT-002 Agent Edit Persists Correctly
 *
 * Preconditions:
 * - smoke-bot from AGT-001 exists (or any Codex agent)
 *
 * Steps:
 * 1. Open the agent profile and click Edit.
 * 2. Change the role text to a distinct value.
 * 3. Change the reasoning effort to `high`.
 * 4. Save and verify the profile shows the updated role text.
 * 5. Verify the profile config grid shows `high` reasoning effort.
 * 6. Verify the API returns the updated values.
 *
 * Expected:
 * - edit dialog opens and accepts changes
 * - saved role text is visible in the profile
 * - reasoning effort is persisted and shown
 * - API and UI agree on the stored values
 */
test.describe('AGT-002', () => {
  /** Actual stored slug (used for API lookups). */
  let agentName: string
  /** Display name shown in the sidebar (used for UI navigation). */
  let agentDisplayName: string

  test.beforeAll(async ({ request }) => {
    const agents = await listAgents(request)
    const existing = agents.find(
      (a) => a.display_name === 'smoke-bot' || a.name === 'smoke-bot' || a.name.startsWith('smoke-bot-')
    )
    if (existing) {
      agentName = existing.name
      agentDisplayName = existing.display_name ?? existing.name
    } else {
      const created = await createAgentApi(request, {
        name: 'agt-002-edit',
        display_name: 'agt-002-edit',
        runtime: 'codex',
        model: 'gpt-5.4-mini',
        reasoningEffort: 'medium',
        description: 'initial role',
      })
      agentName = created.name
      agentDisplayName = 'agt-002-edit'
    }
  })

  test('Agent Edit Persists Correctly @case AGT-002', async ({ page, request }) => {
    await gotoApp(page)

    await test.step('Steps 1–3: Open edit dialog, change role and reasoning effort', async () => {
      await openAgentTab(page, agentDisplayName, 'Profile')
      await page.locator('.profile-toolbar').getByRole('button', { name: 'Edit' }).click()
      const dialog = page.locator('[role="dialog"]')
      await expect(dialog).toBeVisible()
      await dialog.locator('textarea').fill('updated role from agt-002')
      await dialog.locator('[role="combobox"][aria-label="Reasoning"]').click()
      await page.locator('[role="option"]').filter({ hasText: /^High$/ }).click()
    })

    await test.step('Steps 4–5: Save and verify profile reflects changes', async () => {
      const dialog = page.locator('[role="dialog"]')
      await dialog.locator('button:has-text("Save")').click()
      await expect(dialog).toBeHidden({ timeout: 15_000 })
      const roleSection = page.locator('.profile-section').filter({ hasText: '[role::brief]' }).first()
      await expect(roleSection.locator('.profile-role-text')).toContainText('updated role from agt-002')
      await expect(page.locator('.profile-config-grid')).toContainText('high')
    })

    await test.step('Step 6: API returns updated values', async () => {
      const detail = await getAgentDetail(request, agentName)
      expect(detail.agent.reasoningEffort).toBe('high')
    })
  })
})
