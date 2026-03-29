import { test, expect } from '@playwright/test'
import { getWhoami, sendAsUser } from './helpers/api'
import { sendChatMessage } from './helpers/ui'

test.describe('MSG-005', () => {
  test('Chat view stays websocket-driven after initial history bootstrap @case MSG-005', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let historyRequests = 0
    const realtimeConsoleLogs: string[] = []

    page.on('request', (req) => {
      const url = new URL(req.url())
      if (/^\/internal\/agent\/[^/]+\/history$/.test(url.pathname)) {
        historyRequests += 1
      }
    })
    page.on('console', (msg) => {
      const text = msg.text()
      if (text.includes('[chorus:realtime] recv')) {
        realtimeConsoleLogs.push(text)
      }
    })

    await page.goto('/', { waitUntil: 'networkidle' })
    await expect(page.locator('.chat-header-name')).toContainText('#all')
    await page.waitForTimeout(1_000)

    const baselineHistoryRequests = historyRequests

    const localToken = `msg-local-${Date.now()}`
    await sendChatMessage(page, localToken)
    await expect(page.locator('.message-item').filter({ hasText: localToken }).first()).toBeVisible()
    expect(historyRequests).toBe(baselineHistoryRequests)

    const remoteToken = `msg-remote-${Date.now()}`
    await sendAsUser(request, username, '#all', remoteToken)
    await expect(page.locator('.message-item').filter({ hasText: remoteToken }).first()).toBeVisible()
    expect(historyRequests).toBe(baselineHistoryRequests)
    expect(realtimeConsoleLogs.length).toBeGreaterThan(0)

    await page.waitForTimeout(4_000)
    expect(historyRequests).toBe(baselineHistoryRequests)
  })
})
