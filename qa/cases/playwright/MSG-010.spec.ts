import { test, expect } from './helpers/fixtures'
import type { Locator } from '@playwright/test'
import {
  createAgentApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
  listChannelsApi,
  sendAsUser,
} from './helpers/api'
import { createUserChannelViaUi } from './helpers/ui'

async function readUnreadCount(locator: Locator): Promise<number> {
  if (await locator.count() === 0) return 0
  const text = (await locator.textContent())?.trim() ?? '0'
  return Number(text)
}

test.describe('MSG-010', () => {
  test('Inactive rooms update unread badges without fetching history, even off the Chat tab @case MSG-010', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      const created = await createAgentApi(request, {
        name: `msg010-bot-${Date.now()}`,
        runtime: 'codex',
        model: 'gpt-5.4-mini',
      })
      agentName = created.name
    }
    const channelName = `qa-unread-${Date.now()}`

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.chat-header-name').waitFor({ state: 'visible', timeout: 30_000 })
    await createUserChannelViaUi(page, channelName, 'inactive unread badge regression')

    const created = (await listChannelsApi(request, { member: username, includeDm: true, includeSystem: true }))
      .find((channel) => channel.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    const readCursorPosts: Array<{ lastReadSeq?: number }> = []
    let historyRequests = 0
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        url.pathname === `/api/conversations/${created!.id}/read-cursor`
      ) {
        readCursorPosts.push(req.postDataJSON() as { lastReadSeq?: number })
      }
      if (
        req.method() === 'GET' &&
        url.pathname === `/api/conversations/${created!.id}/messages`
      ) {
        historyRequests += 1
      }
    })

    const baselineTokens = Array.from({ length: 3 }, (_, index) => `baseline-${Date.now()}-${index + 1}`)
    let baselineLastSeq = 0
    for (const token of baselineTokens) {
      const ack = await sendAsUser(request, agentName, `#${channelName}`, token, {
        suppressAgentDelivery: true,
      })
      baselineLastSeq = ack.seq
    }

    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await expect(page.locator('.message-item').filter({ hasText: baselineTokens.at(-1)! }).first()).toBeVisible()
    await expect
      .poll(
        () =>
          readCursorPosts.find((post) => (post.lastReadSeq ?? 0) >= baselineLastSeq)
            ?.lastReadSeq ?? 0,
        { timeout: 10_000 }
      )
      .toBeGreaterThanOrEqual(baselineLastSeq)

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const baselineHistoryRequests = historyRequests
    const unreadTokens = Array.from(
      { length: 5 },
      (_, index) => `inactive-unread-${index + 1}-${Date.now()}`
    )
    for (const token of unreadTokens) {
      await sendAsUser(request, agentName, `#${channelName}`, token, {
        suppressAgentDelivery: true,
      })
    }

    const channelRow = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(channelRow.locator('.sidebar-unread-badge')).toHaveText(String(unreadTokens.length))
    expect(historyRequests).toBe(baselineHistoryRequests)

    await channelRow.locator('.sidebar-channel-button').click()
    await expect(page.locator('.tasks-panel-channel')).toContainText(`#${channelName}`)
    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await expect(page.locator('.message-item').filter({ hasText: unreadTokens[0] }).first()).toBeVisible()
    // Scroll last unread into viewport so read cursor advances past all unread messages
    await page.locator('.message-item').filter({ hasText: unreadTokens.at(-1)! }).first().scrollIntoViewIfNeeded()
    await expect
      .poll(
        () => readUnreadCount(channelRow.locator('.sidebar-unread-badge')),
        { timeout: 10_000 }
      )
      .toBe(0)
    await expect(page.locator('.message-item').filter({ hasText: unreadTokens.at(-1)! }).first()).toBeVisible()
    await expect(channelRow.locator('.sidebar-unread-badge')).toHaveCount(0)
  })
})
