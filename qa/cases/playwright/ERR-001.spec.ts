import { test, expect } from './helpers/fixtures'
import path from 'node:path'
import { clickSidebarChannel , gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/messaging.md` — ERR-001 Error Surfacing And Recovery
 */
test.describe('ERR-001', () => {
  test('Error Surfacing And Recovery @case ERR-001', async ({ page }) => {
    const fixture = path.resolve(__dirname, '../../fixtures/qa-attachment.txt')
    await gotoApp(page)
    await clickSidebarChannel(page, 'all')

    await test.step('Steps 1–3: Trigger upload failure and verify visible error', async () => {
      await page.route('**/internal/agent/*/upload', async (route) => {
        await route.fulfill({
          status: 500,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'forced upload failure for ERR-001' }),
        })
      })
      const [chooser] = await Promise.all([
        page.waitForEvent('filechooser'),
        page.locator('.attach-btn').click(),
      ])
      await chooser.setFiles(fixture)
      await page.locator('.message-input-send').click()
      await expect(page.locator('.message-input-area')).toContainText('forced upload failure')
      await expect(page.locator('.file-chip')).toContainText('qa-attachment.txt')
    })

    await test.step('Steps 4–5: Clear failed state and verify normal send still works', async () => {
      await page.unroute('**/internal/agent/*/upload')
      await page.locator('.file-chip button').click()
      await page.locator('.message-input-textarea').fill('ERR-001 recovery message')
      await page.locator('.message-input-send').click()
      await expect(page.locator('.message-item').filter({ hasText: 'ERR-001 recovery message' }).first()).toBeVisible()
    })
  })
})
