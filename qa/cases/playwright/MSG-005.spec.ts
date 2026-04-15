import { test, expect } from './helpers/fixtures'
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

    page.on('request', (req) => {
      const url = new URL(req.url())
      if (req.method() === 'GET' && /^\/api\/conversations\/[^/]+\/messages$/.test(url.pathname)) {
        historyRequests += 1
        const after = url.searchParams.get('after')
        historyAfterParams.push(after == null ? null : Number(after))
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    // Let the initial history bootstrap and any immediate gap-fill requests settle
    // before snapshotting the baseline request count.
    await expect(page.locator('.message-input-textarea')).toBeVisible()
    await page.waitForTimeout(500)

    const baselineHistoryRequests = historyRequests

    const localToken = `msg-local-${Date.now()}`
    await sendChatMessage(page, localToken)
    await expect(page.locator('.message-item').filter({ hasText: localToken }).first()).toBeVisible()
    expect(historyRequests).toBe(baselineHistoryRequests)
    const historyAfterLocalSend = historyRequests

    const remoteToken = `msg-remote-${Date.now()}`
    await sendAsUser(request, username, `#${channelName}`, remoteToken)
    await expect(page.locator('.message-item').filter({ hasText: remoteToken }).first()).toBeVisible()
    expect(historyRequests).toBe(historyAfterLocalSend)
    const historyAfterRemoteSend = historyRequests

    // Observe for 2 s to confirm no further history polling occurs
    await page.waitForTimeout(2_000)
    expect(historyRequests).toBe(historyAfterRemoteSend)
    expect(historyAfterParams.slice(baselineHistoryRequests)).toHaveLength(0)
  })
})
