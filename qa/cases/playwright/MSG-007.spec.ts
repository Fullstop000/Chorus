import { test, expect } from './helpers/fixtures'
import { createChannelApi, getWhoami } from './helpers/api'
import { clickSidebarChannel, openThreadFromMessage, sendChatMessage, sendThreadMessage } from './helpers/ui'

test.describe('MSG-007', () => {
  test('main chat keeps optimistic rows through success and failure @case MSG-007', async ({
    page,
    request,
  }) => {
    const channelName = `msg007-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-007 optimistic main chat coverage',
    })
    let sendCount = 0
    await page.route(`**/api/conversations/${channel.id}/messages`, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      sendCount += 1
      if (sendCount === 1) {
        await new Promise((resolve) => setTimeout(resolve, 500))
        await route.continue()
        return
      }
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'forced send failure' }),
      })
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    const successToken = `optimistic-main-ok-${Date.now()}`
    await sendChatMessage(page, successToken)
    const successMessage = page.locator('.message-item').filter({ hasText: successToken }).first()
    await expect(successMessage).toBeVisible()
    await expect(successMessage.locator('.message-status-sending')).toBeVisible()
    await expect(successMessage.locator('.message-status-sending')).toBeHidden({ timeout: 10_000 })
    await expect(successMessage).toBeVisible()

    const failureToken = `optimistic-main-fail-${Date.now()}`
    await sendChatMessage(page, failureToken)
    const failedMessage = page.locator('.message-item').filter({ hasText: failureToken }).first()
    await expect(failedMessage).toBeVisible()
    await expect(failedMessage.locator('.message-status-failed')).toBeVisible({ timeout: 10_000 })
    await expect(page.locator('.toast-card')).toContainText('Message failed to send')
  })

  test('thread composer keeps optimistic rows through success and failure', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    const channelName = `msg007-thread-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-007 optimistic thread coverage',
    })
    const parentToken = `optimistic-thread-parent-${Date.now()}`
    const parentResponse = await request.post(`/internal/agent/${encodeURIComponent(username)}/send`, {
      data: { target: `#${channelName}`, content: parentToken },
    })
    expect(parentResponse.ok(), await parentResponse.text()).toBeTruthy()

    let sendCount = 0
    await page.route(`**/api/conversations/${channel.id}/messages`, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      sendCount += 1
      if (sendCount === 1) {
        await new Promise((resolve) => setTimeout(resolve, 500))
        await route.continue()
        return
      }
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'forced thread failure' }),
      })
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await openThreadFromMessage(page, parentToken)

    const successToken = `optimistic-thread-ok-${Date.now()}`
    await sendThreadMessage(page, successToken)
    const successMessage = page.locator('.thread-panel .message-item').filter({ hasText: successToken }).first()
    await expect(successMessage).toBeVisible()
    await expect(successMessage.locator('.message-status-sending')).toBeVisible()
    await expect(successMessage.locator('.message-status-sending')).toBeHidden({ timeout: 10_000 })
    await expect(successMessage).toBeVisible()

    const failureToken = `optimistic-thread-fail-${Date.now()}`
    await sendThreadMessage(page, failureToken)
    const failedMessage = page.locator('.thread-panel .message-item').filter({ hasText: failureToken }).first()
    await expect(failedMessage).toBeVisible()
    await expect(failedMessage.locator('.message-status-failed')).toBeVisible({ timeout: 10_000 })
    await expect(page.locator('.toast-card')).toContainText('Message failed to send')
  })
})
