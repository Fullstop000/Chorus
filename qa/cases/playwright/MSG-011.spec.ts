import { test, expect } from '@playwright/test'
import { createChannelApi } from './helpers/api'
import { clickSidebarChannel, sendChatMessage } from './helpers/ui'

test.describe('MSG-011', () => {
  test('Failed sends stay visible and can be retried from the message row @case MSG-011', async ({
    page,
    request,
  }) => {
    const channelName = `msg011-${Date.now()}`
    await createChannelApi(request, {
      name: channelName,
      description: 'MSG-011 retryable failed sends',
    })
    let sendAttempts = 0
    page.route('**/internal/agent/*/send', async (route) => {
      sendAttempts += 1
      if (sendAttempts === 1) {
        await route.fulfill({
          status: 500,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'forced send failure' }),
        })
        return
      }
      await route.continue()
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    const token = `retry-${Date.now()}`
    await sendChatMessage(page, token)

    const failedMessage = page.locator('.message-item').filter({ hasText: token }).first()
    await expect(failedMessage).toBeVisible()
    await expect(failedMessage.locator('.message-status-failed')).toBeVisible()
    await expect(page.locator('.toast-card').filter({ hasText: 'Message failed to send' }).first()).toBeVisible()

    await failedMessage.getByRole('button', { name: 'Retry send' }).click()

    await expect(failedMessage.locator('.message-status-failed')).toBeHidden()
    await expect(failedMessage.locator('.message-status-sending')).toBeHidden()
    await expect(page.locator('.message-item').filter({ hasText: token })).toHaveCount(1)
    expect(sendAttempts).toBe(2)
  })
})
