import { test, expect } from './helpers/fixtures'
import { createUserChannelViaUi, clickSidebarChannel , gotoApp } from './helpers/ui'

test.describe('CHN-005', () => {
  test('Channel rename updates sidebar immediately without full refresh @case CHN-005', async ({
    page,
  }) => {
    const original = `qa-rename-${Date.now()}`
    const renamed = `qa-retitled-${Date.now()}`

    await gotoApp(page)

    await createUserChannelViaUi(page, original, 'playwright CHN-005')
    await clickSidebarChannel(page, original)
    await expect(page.locator('.chat-header-name')).toContainText(`#${original}`)

    const row = page.locator('.sidebar-channel-row').filter({ hasText: original }).first()
    await row.hover()
    await row.locator('button[title^="Edit #"]').click()

    const dialog = page.locator('[role="dialog"]')
    await expect(dialog.getByRole('heading', { name: 'Edit Channel' })).toBeVisible()
    await dialog.locator('input').first().fill(renamed)
    await dialog.locator('button:has-text("Save Changes")').click()

    await expect(dialog).toBeHidden()
    await expect(
      page.locator('.sidebar-item-text').filter({ hasText: new RegExp(`^${renamed}$`) }).first()
    ).toBeVisible()
    await expect(
      page.locator('.sidebar-item-text').filter({ hasText: new RegExp(`^${original}$`) })
    ).toHaveCount(0)
    await expect(page.locator('.chat-header-name')).toContainText(`#${renamed}`)
  })
})
