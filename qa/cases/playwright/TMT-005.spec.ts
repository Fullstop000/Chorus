import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, createTeamApi, teamExists, type TrioNames } from './helpers/api'
import { clickSidebarChannel, openMembersPanel, sendChatMessage , gotoApp , reloadApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-005 Team Member Management (Add, Remove, Role)
 */
let trio: TrioNames

test.describe('TMT-005', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
    if (!(await teamExists(request, 'qa-eng'))) {
      await createTeamApi(request, {
        name: 'qa-eng',
        display_name: 'QA Engineering',
        collaboration_model: 'leader_operators',
        leader_agent_name: trio.botA,
        members: [{ member_name: trio.botA, member_type: 'agent', member_id: trio.botA, role: 'operator' }],
      })
    }
  })

  test('Team Member Add / Remove @case TMT-005', async ({ page }) => {
    test.setTimeout(300_000)

    await gotoApp(page)
    await clickSidebarChannel(page, 'qa-eng')

    await test.step('Step 1: Open team settings', async () => {
      await page.getByRole('button', { name: 'Open team settings' }).click()
      await expect(page.locator('[role="dialog"]').getByRole('heading', { name: 'Team Settings' })).toBeVisible()
    })

    await test.step('Steps 2–3: Add bot-b if missing', async () => {
      const row = page.locator('.team-settings-member').filter({ hasText: 'bot-b' })
      if (!(await row.isVisible().catch(() => false))) {
        await page.locator('[role="dialog"] [role="combobox"][aria-label="Add Member"]').click()
        await page.locator('[role="option"]').filter({ hasText: 'bot-b' }).first().click()
        await page.locator('.team-settings-add-row button:has-text("Add")').click()
        await page.locator('[role="dialog"] button:has-text("Save")').click()
      }
      await expect(page.locator('.team-settings-member').filter({ hasText: 'bot-b' })).toBeVisible()
    })

    await test.step('Step 4: Members rail lists bot-b', async () => {
      await page.locator('[role="dialog"] button:has-text("Close")').click()
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
        await page.locator('.team-settings-card button:has-text("Save")').click()
      }
      await expect(page.locator('.team-settings-member').filter({ hasText: 'bot-b' })).toHaveCount(0)
      await page.locator('[role="dialog"] button:has-text("Close")').click()
    })

    await test.step('Steps 8–10: Members rail without bot-b; refresh', async () => {
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-name').filter({ hasText: 'bot-b' })).toHaveCount(0)
      await reloadApp(page)
      await clickSidebarChannel(page, 'qa-eng')
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-name').filter({ hasText: 'bot-b' })).toHaveCount(0)
    })
  })
})
