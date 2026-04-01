import { test, expect } from './helpers/fixtures'
import type { APIRequestContext, Locator } from '@playwright/test'
import {
  createAgentApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
  listChannelsApi,
} from './helpers/api'
import { createUserChannelViaUi } from './helpers/ui'

async function postMessage(
  request: APIRequestContext,
  actor: string,
  target: string,
  content: string,
  options?: { suppressAgentDelivery?: boolean }
): Promise<{ messageId: string; seq: number }> {
  const response = await request.post(`/internal/agent/${encodeURIComponent(actor)}/send`, {
    data: {
      target,
      content,
      suppressAgentDelivery: options?.suppressAgentDelivery ?? false,
    },
  })
  expect(response.ok(), await response.text()).toBeTruthy()
  return response.json()
}

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
      agentName = `msg010-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `qa-unread-${Date.now()}`

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.chat-header-name').waitFor({ state: 'visible', timeout: 30_000 })
    await createUserChannelViaUi(page, channelName, 'inactive unread badge regression')

    const created = (await listChannelsApi(request, { member: username, includeDm: true, includeSystem: true }))
      .find((channel) => channel.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    const readCursorPosts: Array<{ threadParentId?: string; lastReadSeq?: number }> = []
    let historyRequests = 0
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        url.pathname === `/api/conversations/${created!.id}/read-cursor`
      ) {
        readCursorPosts.push(req.postDataJSON() as { threadParentId?: string; lastReadSeq?: number })
      }
      if (
        req.method() === 'GET' &&
        url.pathname === `/api/conversations/${created!.id}/messages`
      ) {
        historyRequests += 1
      }
    })

    const baselineTokens = Array.from({ length: 12 }, (_, index) => `baseline-${Date.now()}-${index + 1}`)
    let baselineLastSeq = 0
    for (const token of baselineTokens) {
      const ack = await postMessage(request, agentName, `#${channelName}`, token, {
        suppressAgentDelivery: true,
      })
      baselineLastSeq = ack.seq
    }

    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await expect(page.locator('.message-item').filter({ hasText: baselineTokens.at(-1)! }).first()).toBeVisible()
    await expect
      .poll(
        () =>
          readCursorPosts.find(
            (post) => !post.threadParentId && (post.lastReadSeq ?? 0) >= baselineLastSeq
          )?.lastReadSeq ?? 0,
        { timeout: 10_000 }
      )
      .toBeGreaterThanOrEqual(baselineLastSeq)

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const baselineHistoryRequests = historyRequests
    const unreadTokens = Array.from(
      { length: 30 },
      (_, index) => `inactive-unread-${index + 1}-${Date.now()} ${'x'.repeat(120)}`
    )
    for (const token of unreadTokens) {
      await postMessage(request, agentName, `#${channelName}`, token, {
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
    await expect
      .poll(
        () => page.locator('.chat-messages').evaluate((node) => Math.round((node as HTMLElement).scrollTop)),
        { timeout: 10_000 }
      )
      .toBeGreaterThan(0)
    await expect
      .poll(
        () => readUnreadCount(channelRow.locator('.sidebar-unread-badge')),
        { timeout: 10_000 }
      )
      .toBeGreaterThan(0)
    await expect
      .poll(
        () => readUnreadCount(channelRow.locator('.sidebar-unread-badge')),
        { timeout: 10_000 }
      )
      .toBeLessThan(unreadTokens.length)
    await page.locator('.chat-messages').evaluate((node) => {
      const element = node as HTMLElement
      element.scrollTop = element.scrollHeight
    })
    await expect(page.locator('.message-item').filter({ hasText: unreadTokens.at(-1)! }).first()).toBeVisible()
    await expect(channelRow.locator('.sidebar-unread-badge')).toHaveCount(0)
  })
})
