import type { APIRequestContext } from '@playwright/test'
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
  content: string
): Promise<{ messageId: string }> {
  const response = await request.post(`/internal/agent/${encodeURIComponent(actor)}/send`, {
    data: { target, content },
  })
  expect(response.ok(), await response.text()).toBeTruthy()
  return response.json()
}

test.describe('MSG-006', () => {
  test('thread read cursor is only sent after thread replies become visible @case MSG-006', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `msg006-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `msg006-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-006 thread read cursor coverage',
    })
    await inviteChannelMemberApi(request, channel.id, agentName)
    const parentToken = `thread-parent-${Date.now()}`
    const replyToken = `thread-reply-${Date.now()}`
    const parent = await postMessage(request, username, `#${channelName}`, parentToken)
    await postMessage(request, agentName, `#${channelName}:${parent.messageId}`, replyToken)

    const readCursorPosts: Array<{ target?: string; lastReadSeq?: number }> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        /^\/internal\/agent\/[^/]+\/read-cursor$/.test(url.pathname)
      ) {
        const payload = req.postDataJSON() as { target?: string; lastReadSeq?: number }
        readCursorPosts.push(payload)
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
    await page.waitForTimeout(1_000)

    expect(
      readCursorPosts.some((post) => post.target === `#${channelName}:${parent.messageId}`)
    ).toBeFalsy()

    await parentMessage.locator('.message-reply-count').click()
    await expect(page.locator('.thread-panel')).toBeVisible()
    await expect(page.locator('.thread-panel .message-item').filter({ hasText: replyToken })).toBeVisible()

    await expect
      .poll(
        () =>
          readCursorPosts.find((post) => post.target === `#${channelName}:${parent.messageId}`)?.lastReadSeq ??
          null,
        { timeout: 10_000 }
      )
      .not.toBeNull()
  })
})
