import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, createTeamApi, teamExists } from './helpers/api'
import { clickSidebarChannel, openMembersPanel, sendChatMessage } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-005 Team Member Management (Add, Remove, Role)
 *
 * Preconditions:
 * - team `qa-eng` with bot-a; bot-b exists and may be added
 *
 * Steps:
 * 1–4. Settings: add bot-b; verify settings + members rail.
 * 5. Post in `#qa-eng`; bot-b responds (LLM).
 * 6–8. Remove bot-b; verify lists.
 * 9. Post again; bot-b does not respond (heuristic / LLM).
 * 10. Refresh; state consistent.
 */
test.describe('TMT-005', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
    if (!(await teamExists(request, 'qa-eng'))) {
      await createTeamApi(request, {
        name: 'qa-eng',
        display_name: 'QA Engineering',
        collaboration_model: 'leader_operators',
        leader_agent_name: 'bot-a',
        members: [{ member_name: 'bot-a', member_type: 'agent', member_id: 'bot-a', role: 'operator' }],
      })
    }
  })

  test('Team Member Add / Remove @case TMT-005', async ({ page }) => {
    test.setTimeout(300_000)

    await page.goto('/', { waitUntil: 'networkidle' })
    await clickSidebarChannel(page, 'qa-eng')

    await test.step('Step 1: Open team settings', async () => {
      await page.getByRole('button', { name: 'Open team settings' }).click()
      await expect(page.locator('.modal-title:text("Team Settings")')).toBeVisible()
    })

    await test.step('Steps 2–3: Add bot-b if missing', async () => {
      const row = page.locator('.team-settings-member').filter({ hasText: 'bot-b' })
      if (!(await row.isVisible().catch(() => false))) {
        await page.locator('.team-settings-add-row select').selectOption('bot-b')
        await page.locator('.team-settings-add-row button:has-text("Add")').click()
      }
      await expect(page.locator('.team-settings-member').filter({ hasText: 'bot-b' })).toBeVisible()
    })

    await test.step('Step 4: Members rail lists bot-b', async () => {
      await page.locator('.team-settings-card .modal-close').click()
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-name').filter({ hasText: 'bot-b' })).toBeVisible()
      await page.locator('.members-panel-close').click()
    })

    if (!skipLLM) {
      await test.step('Step 5: Message in #qa-eng (bot-b wakeup)', async () => {
        const token = `tmt5-${Date.now()}`
        await sendChatMessage(page, `bot-b acknowledge ${token}`)
      })
    }

    await test.step('Steps 6–7: Remove bot-b from settings', async () => {
      await page.getByRole('button', { name: 'Open team settings' }).click()
      const row = page.locator('.team-settings-member').filter({ hasText: 'bot-b' })
      if (await row.isVisible().catch(() => false)) {
        await row.getByRole('button', { name: 'Remove' }).click()
        await page.waitForTimeout(600)
      }
      await expect(page.locator('.team-settings-member').filter({ hasText: 'bot-b' })).toHaveCount(0)
      await page.locator('.team-settings-card .modal-close').click()
    })

    await test.step('Steps 8–10: Members rail without bot-b; refresh', async () => {
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-name').filter({ hasText: 'bot-b' })).toHaveCount(0)
      await page.reload({ waitUntil: 'networkidle' })
      await clickSidebarChannel(page, 'qa-eng')
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-name').filter({ hasText: 'bot-b' })).toHaveCount(0)
    })
  })
})
