import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, createTeamApi, getWhoami, historyForUser, sendAsUser, teamExists } from './helpers/api'
import { clickSidebarChannel , gotoApp , reloadApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-006 Team Settings Update (Display Name, Collaboration Model, Leader)
 *
 * Preconditions:
 * - team `qa-eng` leader_operators, leader bot-a
 *
 * Steps:
 * 1. Open team settings.
 * 2–3. Display name → `QA Engineering v2`, save, verify in panel.
 * 4–5. Switch to Swarm, save, reopen → shows Swarm.
 * 6. `@qa-eng` task → deliberation (LLM).
 * 7–8. Back to Leader+Operators, leader bot-b (LLM routing).
 * 9. Refresh — persist.
 *
 * This automation covers 1–5 and 9 via UI/API. Steps 6–8 are partially covered when LLM enabled.
 */
test.describe('TMT-006', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
    if (!(await teamExists(request, 'qa-eng'))) {
      await createTeamApi(request, {
        name: 'qa-eng',
        display_name: 'QA Engineering',
        collaboration_model: 'leader_operators',
        leader_agent_name: 'bot-a',
        members: [
          { member_name: 'bot-a', member_type: 'agent', member_id: 'bot-a', role: 'operator' },
          { member_name: 'bot-b', member_type: 'agent', member_id: 'bot-b', role: 'operator' },
        ],
      })
    } else {
      // Ensure bot-b is a member for this test
      await request.post('/api/teams/qa-eng/members', {
        data: { member_name: 'bot-b', member_type: 'agent', member_id: 'bot-b', role: 'operator' },
      }).catch(() => {})
    }
  })

  test('Team settings display name + model toggle @case TMT-006', async ({ page, request }) => {
    test.setTimeout(300_000)

    await gotoApp(page)
    await clickSidebarChannel(page, 'qa-eng')

    await test.step('Step 1: Open settings', async () => {
      await page.getByRole('button', { name: 'Open team settings' }).click()
    })

    const dialog = page.locator('[role="dialog"]')

    await test.step('Steps 2–3: Display name QA Engineering v2 + Save', async () => {
      await dialog.locator('input').first().fill('QA Engineering v2')
      await dialog.locator('button:has-text("Save")').click()
      await expect(dialog.locator('input').first()).toHaveValue(/QA Engineering v2/)
    })

    const collabTrigger = dialog.locator('[role="combobox"][aria-label="Collaboration Model"]')

    await test.step('Steps 4–5: Collaboration model Swarm + save + reopen', async () => {
      await collabTrigger.click()
      await page.locator('[role="option"]').filter({ hasText: 'Swarm' }).click()
      await dialog.locator('button:has-text("Save")').click()
      await dialog.locator('button:has-text("Close")').click()
      await page.getByRole('button', { name: 'Open team settings' }).click()
      await expect(collabTrigger).toContainText('Swarm')
    })

    if (!skipLLM) {
      await test.step('Step 6: Forward task — expect swarm deliberation line', async () => {
        const { username } = await getWhoami(request)
        const mark = `tmt6-${Date.now()}`
        await sendAsUser(request, username, '#all', `@qa-eng do something ${mark}`)
        await new Promise((r) => setTimeout(r, 35_000))
        const msgs = await historyForUser(request, 'bot-a', '#qa-eng', 40)
        const deliberation = msgs.some(
          (m) =>
            (m.senderType === 'system' || m.senderName === 'system') &&
            (m.content ?? '').includes('Discuss the best approach')
        )
        expect(deliberation).toBe(true)
      })
    }

    await test.step('Steps 7–8 (partial): Restore Leader+Operators, leader bot-b', async () => {
      await collabTrigger.click()
      await page.locator('[role="option"]').filter({ hasText: 'Leader+Operators' }).click()
      // Wait for React to re-render the conditionally-shown leader select
      const leaderTrigger = dialog.locator('[role="combobox"][aria-label="Leader"]')
      await expect(leaderTrigger).toBeVisible()
      await leaderTrigger.click()
      await page.locator('[role="option"]').filter({ hasText: 'bot-b' }).click()
      await dialog.locator('button:has-text("Save")').click()
    })

    await test.step('Step 9: Refresh — reopen settings', async () => {
      await dialog.locator('button:has-text("Close")').click()
      await reloadApp(page)
      await clickSidebarChannel(page, 'qa-eng')
      await page.getByRole('button', { name: 'Open team settings' }).click()
      await expect(collabTrigger).toContainText('Leader+Operators')
    })
  })
})
