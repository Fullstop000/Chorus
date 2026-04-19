import type { APIRequestContext } from '@playwright/test'
import { test, expect } from './helpers/fixtures'
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
): Promise<{ messageId: string; seq: number }> {
  const response = await request.post(`/internal/agent/${encodeURIComponent(actor)}/send`, {
    data: { target, content },
  })
  expect(response.ok(), await response.text()).toBeTruthy()
  return response.json()
}

test.describe('MSG-008', () => {
  test('conversation read cursor advances after visible top-level messages render @case MSG-008', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `msg008-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }

    const channelName = `msg008-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-008 conversation read cursor coverage',
    })
    await inviteChannelMemberApi(request, channel.id, agentName)

    const token = `conversation-read-${Date.now()}`
    const seeded = await postMessage(request, agentName, `#${channelName}`, token)

    const readCursorPosts: Array<{ lastReadSeq?: number }> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        url.pathname === `/api/conversations/${channel.id}/read-cursor`
      ) {
        const payload = req.postDataJSON() as { lastReadSeq?: number }
        readCursorPosts.push(payload)
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.chat-header-name').waitFor({ state: 'visible', timeout: 30_000 })
    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()

    await expect
      .poll(
        () =>
          readCursorPosts.find((post) => (post.lastReadSeq ?? 0) >= seeded.seq)?.lastReadSeq ??
          null,
        { timeout: 10_000 }
      )
      .not.toBeNull()
  })
})
