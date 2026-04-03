import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio } from './helpers/api'
import { openAgentChat, sendChatMessage, gotoApp } from './helpers/ui'

test.describe('DM-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('DM chat renders sent message @case DM-001', async ({ page }) => {
    const token = `dm-render-${Date.now()}`

    await gotoApp(page)

    await test.step('Open DM with bot-a', async () => {
      await openAgentChat(page, 'bot-a')
      await expect(page.locator('.chat-header-name')).toContainText('@bot-a', { timeout: 15_000 })
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })

    await test.step('Send a message and verify it renders in DM chatbox', async () => {
      await sendChatMessage(page, token)
      await expect(page.getByText(token).first()).toBeVisible({ timeout: 10_000 })
    })

    await test.step('Verify message appears inside .message-item container', async () => {
      await expect(page.locator('.message-item').filter({ hasText: token })).toBeVisible({ timeout: 5_000 })
    })
  })
})
