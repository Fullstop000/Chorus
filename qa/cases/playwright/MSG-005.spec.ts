import { test, expect } from './helpers/fixtures'
import path from 'node:path'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { clickSidebarChannel , gotoApp } from './helpers/ui'

/**
 * Catalog: qa/cases/messaging.md — MSG-005 Attachment Upload And Download
 * Supersedes: ATT-001
 */
test.describe('MSG-005', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Attachment Upload And Download @case MSG-005', async ({ page, request }) => {
    const fixture = path.resolve(__dirname, '../../fixtures/qa-attachment.txt')
    const { username } = await getWhoami(request)
    await gotoApp(page)
    await clickSidebarChannel(page, 'all')

    await test.step('Steps 1–4: Attach file and send message', async () => {
      const [chooser] = await Promise.all([
        page.waitForEvent('filechooser'),
        page.locator('.attach-btn').click(),
      ])
      await chooser.setFiles(fixture)
      await expect(page.locator('.file-chip')).toContainText('qa-attachment.txt')
      await page.locator('.message-input-send').click()
    })

    await test.step('Steps 5–7: Attachment renders with download target and composer clears', async () => {
      await expect(page.locator('.attachment-link').filter({ hasText: 'qa-attachment.txt' }).first()).toBeVisible()
      await expect(page.locator('.file-chip')).toHaveCount(0)
      const history = await historyForUser(request, username, '#all', 30)
      const msg = history.find((entry) => (entry.attachments ?? []).some((att) => att.filename === 'qa-attachment.txt'))
      expect(msg).toBeTruthy()
    })
  })
})
