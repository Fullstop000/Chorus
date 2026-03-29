import { test, expect } from '@playwright/test'
import { createUserChannelViaUi, clickSidebarChannel } from './helpers/ui'

test.describe('CHN-005', () => {
  test('Channel rename updates sidebar immediately without full refresh @case CHN-005', async ({
    page,
  }) => {
    const original = `qa-rename-${Date.now()}`
    const renamed = `qa-retitled-${Date.now()}`

    await page.goto('/', { waitUntil: 'networkidle' })

    await createUserChannelViaUi(page, original, 'playwright CHN-005')
    await clickSidebarChannel(page, original)
    await expect(page.locator('.chat-header-name')).toContainText(`#${original}`)

    const row = page.locator('.sidebar-channel-row').filter({ hasText: original }).first()
    await row.hover()
    await row.locator('button[title^="Edit #"]').click()

    await expect(page.locator('.modal-title').filter({ hasText: 'Edit Channel' })).toBeVisible()
    await page.locator('.modal-card .form-input').fill(renamed)
    await page.locator('.modal-card button:has-text("Save Changes")').click()

    await expect(page.locator('.modal-title').filter({ hasText: 'Edit Channel' })).toBeHidden()
    await expect(
      page.locator('.sidebar-item-text').filter({ hasText: new RegExp(`^${renamed}$`) }).first()
    ).toBeVisible()
    await expect(
      page.locator('.sidebar-item-text').filter({ hasText: new RegExp(`^${original}$`) })
    ).toHaveCount(0)
    await expect(page.locator('.chat-header-name')).toContainText(`#${renamed}`)
  })
})
