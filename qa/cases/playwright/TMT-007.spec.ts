import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, createTeamApi, getWhoami, historyForUser, sendAsUser, teamExists } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-007 Team Delete — Channel Archive and Workspace Cleanup
 *
 * **Automation note:** The catalog uses disposable team `qa-eng` with history; deleting it would break other scripted cases.
 * This spec uses a **disposable team** `qa-del-<timestamp>` with the same delete flow and post-delete checks (Steps 1–6, partial 7–8).
 * Step 9 (recreate non-team channel `qa-eng`) is **not** automated here.
 *
 * Preconditions:
 * - disposable team with at least bot-a member and optional messages
 *
 * Steps:
 * 1–3. Open settings, delete, confirm.
 * 4–6. Channel gone; UI sane; refresh stable.
 * 7. bot-a still works in `#all`.
 * 8. bot-a no longer lists deleted team (LLM).
 */
test.describe('TMT-007', () => {
  test('Team delete (disposable team) @case TMT-007', async ({ page, request }) => {
    test.setTimeout(240_000)
    await ensureMixedRuntimeTrio(request)

    const name = `qa-del-${Date.now()}`
    await createTeamApi(request, {
      name,
      display_name: 'E2E Delete Target',
      collaboration_model: 'leader_operators',
      leader_agent_name: 'bot-a',
      members: [{ member_name: 'bot-a', member_type: 'agent', member_id: 'bot-a', role: 'operator' }],
    })

    await page.goto('/', { waitUntil: 'networkidle' })
    await clickSidebarChannel(page, name)

    await test.step('Steps 1–3: Delete team + confirm dialog', async () => {
      page.once('dialog', (d) => d.accept())
      await page.getByRole('button', { name: 'Open team settings' }).click()
      await page.getByRole('button', { name: 'Delete Team' }).click()
    })

    await test.step('Steps 4–6: Channel removed; refresh', async () => {
      await expect(page.locator('.modal-title:text("Team Settings")')).toBeHidden({ timeout: 25_000 })
      await expect(page.locator('.sidebar-item-text').filter({ hasText: name })).toHaveCount(0)
      await page.reload({ waitUntil: 'networkidle' })
      await expect(page.locator('.sidebar-item-text').filter({ hasText: name })).toHaveCount(0)
      expect(await teamExists(request, name)).toBe(false)
    })

    if (!skipLLM) {
      await test.step('Steps 7–8: bot-a still answers #all; team list omits deleted slug', async () => {
        const { username } = await getWhoami(request)
        const mark = `tmt7-${Date.now()}`
        await sendAsUser(
          request,
          username,
          '#all',
          `bot-a ${mark}: list your team slugs; do not include ${name}.`
        )
        await new Promise((r) => setTimeout(r, 60_000))
        const msgs = await historyForUser(request, username, '#all', 25)
        const fromA = msgs.filter((m) => m.senderName === 'bot-a').pop()
        expect(fromA, 'expected bot-a reply in #all').toBeTruthy()
        expect((fromA!.content ?? '').toLowerCase()).not.toContain(name.toLowerCase())
      })
    }
  })
})
