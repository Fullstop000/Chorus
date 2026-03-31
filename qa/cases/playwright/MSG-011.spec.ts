import type { APIRequestContext, Locator } from '@playwright/test'
import { test, expect } from '@playwright/test'
import {
  createAgentApi,
  createChannelApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
} from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

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

async function readThreadUnreadCount(row: Locator): Promise<number> {
  const badge = row.locator('.threads-tab__unread')
  if (await badge.count() === 0) return 0
  const text = (await badge.textContent())?.trim() ?? '0 unread'
  return Number(text.split(/\s+/)[0] ?? '0')
}

test.describe('MSG-011', () => {
  test('thread unread lifecycle preserves replyCount and clears after reading replies @case MSG-011', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `msg011-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }

    const channelName = `msg011-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-011 thread unread lifecycle coverage',
    })
    await inviteChannelMemberApi(request, channel.id, agentName)

    const parentToken = `thread-parent-${Date.now()}`
    const parent = await postMessage(request, username, `#${channelName}`, parentToken)

    const baselineReplyTokens = Array.from(
      { length: 8 },
      (_, index) => `thread-baseline-${index + 1}-${Date.now()} ${'y'.repeat(80)}`
    )
    let baselineLastSeq = 0
    for (const token of baselineReplyTokens) {
      const ack = await postMessage(request, agentName, `#${channelName}:${parent.messageId}`, token, {
        suppressAgentDelivery: true,
      })
      baselineLastSeq = ack.seq
    }

    const readCursorPosts: Array<{ threadParentId?: string; lastReadSeq?: number }> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        url.pathname === `/api/conversations/${channel.id}/read-cursor`
      ) {
        readCursorPosts.push(req.postDataJSON() as { threadParentId?: string; lastReadSeq?: number })
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    const parentMessage = page.locator('.message-item').filter({ hasText: parentToken }).first()
    await expect(parentMessage).toBeVisible()
    await parentMessage.locator('.message-reply-count').click()
    await expect(page.locator('.thread-panel')).toBeVisible()
    await expect(page.locator('.thread-panel .message-item').filter({ hasText: baselineReplyTokens.at(-1)! }).first()).toBeVisible()
    await page.locator('.thread-body').evaluate((node) => {
      const element = node as HTMLElement
      element.scrollTop = element.scrollHeight
      element.dispatchEvent(new Event('scroll'))
    })
    await expect
      .poll(
        () =>
          readCursorPosts.find(
            (post) => post.threadParentId === parent.messageId && (post.lastReadSeq ?? 0) >= baselineLastSeq
          )?.lastReadSeq ?? 0,
        { timeout: 10_000 }
      )
      .toBeGreaterThanOrEqual(baselineLastSeq)
    await page.locator('.thread-close-btn').click()
    await expect(page.locator('.thread-panel')).toHaveCount(0)

    const unreadReplyTokens = Array.from(
      { length: 24 },
      (_, index) => `thread-unread-${index + 1}-${Date.now()} ${'z'.repeat(140)}`
    )
    for (const token of unreadReplyTokens) {
      await postMessage(request, agentName, `#${channelName}:${parent.messageId}`, token, {
        suppressAgentDelivery: true,
      })
    }

    const totalReplies = baselineReplyTokens.length + unreadReplyTokens.length

    await page.getByRole('button', { name: /Threads/ }).click()
    const threadRow = page.locator('.threads-tab__row').filter({ hasText: parentToken }).first()
    await expect(threadRow).toContainText(`${totalReplies} repl`)
    await expect(threadRow).toContainText(`${unreadReplyTokens.length} unread`)
    await threadRow.click()

    await expect(page.locator('.thread-panel .message-item').filter({ hasText: unreadReplyTokens[0] }).first()).toBeVisible()
    await expect
      .poll(
        () => page.locator('.thread-body').evaluate((node) => Math.round((node as HTMLElement).scrollTop)),
        { timeout: 10_000 }
      )
      .toBeGreaterThan(0)

    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await page.getByRole('button', { name: /Threads/ }).click()
    const refreshedThreadRow = page.locator('.threads-tab__row').filter({ hasText: parentToken }).first()
    await expect(refreshedThreadRow).toContainText(`${totalReplies} repl`)
    await expect
      .poll(() => readThreadUnreadCount(refreshedThreadRow), { timeout: 10_000 })
      .toBeGreaterThan(0)
    await expect
      .poll(() => readThreadUnreadCount(refreshedThreadRow), { timeout: 10_000 })
      .toBeLessThan(unreadReplyTokens.length)

    await refreshedThreadRow.click()
    await page.locator('.thread-body').evaluate((node) => {
      const element = node as HTMLElement
      element.scrollTop = element.scrollHeight
      element.dispatchEvent(new Event('scroll'))
    })
    await expect(page.locator('.thread-panel .message-item').filter({ hasText: unreadReplyTokens.at(-1)! }).first()).toBeVisible()

    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await page.getByRole('button', { name: /Threads/ }).click()
    const finalThreadRow = page.locator('.threads-tab__row').filter({ hasText: parentToken }).first()
    await expect(finalThreadRow).toContainText(`${totalReplies} repl`)
    // Note: Thread unread count clears after the threads list refreshes.
    // This happens automatically when switching back to the Threads tab.
    await expect(finalThreadRow.locator('.threads-tab__unread')).toHaveCount(0)
  })
})
