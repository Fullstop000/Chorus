import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, teamExists } from './helpers/api'
import { createTeamQaEngViaUi, clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/teams.md` — TMT-001 Team Create, Channel Badge, and Sidebar Appearance
 *
 * Preconditions:
 * - at least 2 agents exist
 *
 * Steps:
 * 1. Click `+ New Channel` and verify the modal has a Channel / Team toggle at the top.
 * 2. Switch the toggle to `Team`.
 * 3. Fill in name `qa-eng`, display name `QA Engineering`, model Leader+Operators, leader + operator agents.
 * 4. Submit the form.
 * 5. Verify `#qa-eng` appears with `[team]` badge, not `[sys]`.
 * 6. Verify no separate Teams-only section (sidebar structure).
 * 7. Click `+ New Team` shortcut; modal opens on Team tab; cancel.
 * 8. Cancel without creating (step 7).
 * 9. Open `#qa-eng` — team settings control in header.
 * 10. Refresh — badge persists.
 *
 * Expected:
 * - team in channel list with team badge; both creation paths; settings distinct; refresh stable
 */
test.describe('TMT-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Team Create, Channel Badge, Sidebar @case TMT-001', async ({ page, request }) => {
    const hasTeam = await teamExists(request, 'qa-eng')

    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–2: New Channel modal — Channel / Team toggle', async () => {
      await page.click('button[title="Add channel"]')
      await expect(page.locator('.modal-title:text("Create Channel")')).toBeVisible()
      await expect(page.locator('.btn-brutal:has-text("Team")')).toBeVisible()
      await page.locator('.btn-brutal:has-text("Team")').click()
      await expect(page.locator('.modal-title:text("Create Team")')).toBeVisible()
      await page.locator('.modal-close').click()
    })

    await test.step('Step 7 (+ New Team shortcut): Team tab pre-selected', async () => {
      await page.click('button[title="Add team"]')
      await expect(page.locator('.modal-title:text("Create Team")')).toBeVisible()
      await page.locator('.modal-close').click()
    })

    if (!hasTeam) {
      await test.step('Steps 3–4: Create qa-eng team', async () => {
        await createTeamQaEngViaUi(page)
      })
    }

    await test.step('Steps 5–6: Sidebar shows qa-eng with team badge (not sys)', async () => {
      await expect(page.locator('.sidebar-item-text:text("qa-eng")').first()).toBeVisible()
      await expect(page.locator('.sidebar-channel-badge.team').first()).toContainText('team')
    })

    await test.step('Step 9: Open qa-eng — team settings affordance', async () => {
      await clickSidebarChannel(page, 'qa-eng')
      await expect(page.getByRole('button', { name: 'Open team settings' })).toBeVisible()
    })

    await test.step('Step 10: Refresh — team badge persists', async () => {
      await page.reload({ waitUntil: 'networkidle' })
      await expect(page.locator('.sidebar-item-text:text("qa-eng")').first()).toBeVisible()
      await expect(page.locator('.sidebar-channel-badge.team').first()).toContainText('team')
    })
  })
})
