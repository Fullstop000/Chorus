import { test, expect } from './helpers/fixtures'
import type { Page } from '@playwright/test'
import { ensureMixedRuntimeTrio, ensureStubTrio, teamExists } from './helpers/api'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const useStub = mode === 'stub'
import { createTeamQaEngViaUi, clickSidebarChannel , gotoApp , reloadApp } from './helpers/ui'

async function expectSingleRightAlignedTeamRow(page: Page) {
  const row = page
    .locator('.sidebar-channel-row')
    .filter({ has: page.locator('.sidebar-item-text', { hasText: /^qa-eng$/ }) })

  await expect(row).toHaveCount(1)

  const badge = row.locator('.sidebar-channel-badge.team')
  await expect(badge).toHaveCount(1)
  await expect(badge).toContainText('team')

  const rowBox = await row.boundingBox()
  const badgeBox = await badge.boundingBox()
  expect(rowBox).not.toBeNull()
  expect(badgeBox).not.toBeNull()

  if (!rowBox || !badgeBox) return

  const rightGap = rowBox.x + rowBox.width - (badgeBox.x + badgeBox.width)
  expect(rightGap).toBeLessThanOrEqual(20)
}

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
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
  })

  test('Team Create, Channel Badge, Sidebar @case TMT-001', async ({ page, request }) => {
    const hasTeam = await teamExists(request, 'qa-eng')

    await gotoApp(page)

    await test.step('Steps 1–2: New Channel modal — Channel / Team toggle', async () => {
      await page.click('button[title="Add channel"]')
      const dialog = page.locator('[role="dialog"]')
      await expect(dialog.getByRole('heading', { name: 'Create Channel' })).toBeVisible()
      await expect(dialog.locator('button:has-text("Team")')).toBeVisible()
      await dialog.locator('button:has-text("Team")').click()
      await expect(dialog.getByRole('heading', { name: 'Create Team' })).toBeVisible()
      await dialog.locator('button:has-text("Cancel")').click()
    })

    await test.step('Step 7 (+ New Team shortcut): Team tab pre-selected', async () => {
      await page.click('button[title="Add team"]')
      const dialog = page.locator('[role="dialog"]')
      await expect(dialog.getByRole('heading', { name: 'Create Team' })).toBeVisible()
      await dialog.locator('button:has-text("Cancel")').click()
    })

    if (!hasTeam) {
      await test.step('Steps 3–4: Create qa-eng team', async () => {
        await createTeamQaEngViaUi(page)
      })
    }

    await test.step('Steps 5–6: Sidebar shows exactly one qa-eng row with a right-aligned team badge', async () => {
      await expectSingleRightAlignedTeamRow(page)
      await expect(
        page
          .locator('.sidebar-channel-row')
          .filter({ has: page.locator('.sidebar-item-text', { hasText: /^qa-eng$/ }) })
          .locator('.sidebar-channel-badge')
      ).not.toContainText('sys')
    })

    await test.step('Step 9: Open qa-eng — team settings affordance', async () => {
      await clickSidebarChannel(page, 'qa-eng')
      await expect(page.getByRole('button', { name: 'Open team settings' })).toBeVisible()
    })

    await test.step('Step 10: Refresh — team badge persists', async () => {
      await reloadApp(page)
      await expectSingleRightAlignedTeamRow(page)
    })
  })
})
