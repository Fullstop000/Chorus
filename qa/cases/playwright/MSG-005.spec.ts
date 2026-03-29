import { test, expect } from '@playwright/test'
import { createChannelApi, getWhoami, sendAsUser } from './helpers/api'
import { clickSidebarChannel, sendChatMessage } from './helpers/ui'

test.describe('MSG-005', () => {
  test('Chat view stays websocket-driven after initial history bootstrap @case MSG-005', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    const channelName = `msg005-${Date.now()}`
    await createChannelApi(request, {
      name: channelName,
      description: 'MSG-005 websocket-driven chat coverage',
    })
    let historyRequests = 0
    const historyAfterParams: Array<number | null> = []
    const realtimeConsoleLogs: string[] = []

    page.on('request', (req) => {
      const url = new URL(req.url())
      if (/^\/internal\/agent\/[^/]+\/history$/.test(url.pathname)) {
        historyRequests += 1
        const after = url.searchParams.get('after')
        historyAfterParams.push(after == null ? null : Number(after))
      }
    })
    page.on('console', (msg) => {
      const text = msg.text()
      if (text.includes('[chorus:realtime] recv')) {
        realtimeConsoleLogs.push(text)
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await page.waitForTimeout(1_000)

    const baselineHistoryRequests = historyRequests
    expect(historyAfterParams.every((value) => value == null)).toBeTruthy()

    const localToken = `msg-local-${Date.now()}`
    await sendChatMessage(page, localToken)
    await expect(page.locator('.message-item').filter({ hasText: localToken }).first()).toBeVisible()
    expect(historyRequests).toBeLessThanOrEqual(baselineHistoryRequests + 1)
    if (historyRequests > baselineHistoryRequests) {
      expect(historyAfterParams.at(-1)).not.toBeNull()
    }
    const historyAfterLocalSend = historyRequests

    const remoteToken = `msg-remote-${Date.now()}`
    await sendAsUser(request, username, `#${channelName}`, remoteToken)
    await expect(page.locator('.message-item').filter({ hasText: remoteToken }).first()).toBeVisible()
    expect(historyRequests).toBe(historyAfterLocalSend + 1)
    expect(historyAfterParams.at(-1)).not.toBeNull()
    const historyAfterRemoteSend = historyRequests
    expect(realtimeConsoleLogs.length).toBeGreaterThan(0)
    const consoleDump = realtimeConsoleLogs.join('\n')
    expect(consoleDump).toContain('conversation.state')
    expect(consoleDump).toContain('latestSeq')
    expect(consoleDump).toContain('unreadCount')
    expect(consoleDump).not.toContain(remoteToken)

    await page.waitForTimeout(4_000)
    expect(historyRequests).toBe(historyAfterRemoteSend)
    expect(historyAfterParams.slice(baselineHistoryRequests).every((value) => value != null)).toBe(
      true
    )
  })
})
