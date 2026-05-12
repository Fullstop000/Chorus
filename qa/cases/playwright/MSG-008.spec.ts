import { test, expect } from './helpers/fixtures'
import {
  createAgentApi,
  createChannelApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
  sendAsUser,
} from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

test.describe('MSG-008', () => {
  test('conversation read cursor advances after visible top-level messages render @case MSG-008', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      // The server appends a random slug suffix to the requested base name,
      // so use the actual returned name for the invite-by-name lookup below.
      const created = await createAgentApi(request, {
        name: `msg008-bot-${Date.now()}`,
        runtime: 'claude',
        model: 'sonnet',
      })
      agentName = created.name
    }

    const channelName = `msg008-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-008 conversation read cursor coverage',
    })
    await inviteChannelMemberApi(request, channel.id, agentName)

    const token = `conversation-read-${Date.now()}`
    const seeded = await sendAsUser(request, agentName, `#${channelName}`, token)

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
