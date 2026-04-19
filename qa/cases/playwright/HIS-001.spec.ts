import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { clickSidebarChannel, openAgentChat, sendChatMessage, gotoApp, reloadApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/messaging.md` — HIS-001 History Reload And Selection Stability
 */
test.describe('HIS-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('History Reload And Selection Stability @case HIS-001', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const mark = `his-${Date.now()}`
    await gotoApp(page)

    await test.step('Precondition: create channel and DM history', async () => {
      await clickSidebarChannel(page, 'all')
      await sendChatMessage(page, `Channel history ${mark}`)
      await openAgentChat(page, 'bot-a')
      await sendChatMessage(page, `DM history ${mark}`)
    })

    await test.step('Steps 1–5: Refresh and verify channel and DM history remain stable', async () => {
      await reloadApp(page)
      await clickSidebarChannel(page, 'all')
      await expect(page.locator('.chat-header-name')).toContainText('#all')
      await expect(page.locator('.message-item').filter({ hasText: `Channel history ${mark}` }).first()).toBeVisible()
      await openAgentChat(page, 'bot-a')
      await expect(page.locator('.message-item').filter({ hasText: `DM history ${mark}` }).first()).toBeVisible()
      const channelHistory = await historyForUser(request, username, `#all`, 50)
      expect(channelHistory.some((m) => (m.content ?? '').includes(`Channel history ${mark}`))).toBe(true)
    })
  })
})
