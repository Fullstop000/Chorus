import { test, expect } from './helpers/fixtures'
import {
  createAgentApi,
  createChannelApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
} from './helpers/api'
import { clickSidebarChannel, openAgentChat } from './helpers/ui'

test.describe('MSG-009', () => {
  test('switching channel and dm keeps one realtime websocket tunnel @case MSG-009', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    const agents = await listAgents(request)
    let agentName = agents[0]?.name
    let displayName = agents[0]?.display_name ?? agentName
    if (!agentName) {
      const inputName = `msg009-bot-${Date.now()}`
      const created = await createAgentApi(request, {
        name: inputName,
        runtime: 'codex',
        model: 'gpt-5.4-mini',
      })
      agentName = created.name
      displayName = inputName
    }

    const channelName = `msg009-${Date.now()}`
    const channel = await createChannelApi(request, {
      name: channelName,
      description: 'MSG-009 single websocket tunnel coverage',
    })
    await inviteChannelMemberApi(request, channel.id, agentName)

    const realtimeSockets: string[] = []
    page.on('websocket', (ws) => {
      if (ws.url().includes('/api/events/ws')) {
        realtimeSockets.push(ws.url())
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().waitFor({
      state: 'visible',
      timeout: 30_000,
    })

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    await openAgentChat(page, displayName)
    await expect(page.locator('.chat-header-name')).toContainText(`@${displayName}`)

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    // Any spurious reconnect would happen within milliseconds — 300 ms is sufficient
    await page.waitForTimeout(300)
    expect(realtimeSockets.length).toBe(1)
    expect(realtimeSockets[0]).toContain(`/api/events/ws?viewer=${encodeURIComponent(username)}`)
  })
})
