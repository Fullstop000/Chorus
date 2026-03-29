import { test, expect } from '@playwright/test'
import { createAgentApi, getWhoami, listAgents, sendAsUser } from './helpers/api'

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

    const createResponse = await request.post('/api/channels', {
      data: { name: channelName, description: 'inactive unread badge regression' },
    })
    expect(createResponse.ok(), await createResponse.text()).toBeTruthy()
    const created = await createResponse.json()

    const inviteResponse = await request.post(`/api/channels/${encodeURIComponent(created.id)}/members`, {
      data: { memberName: agentName },
    })
    expect(inviteResponse.ok(), await inviteResponse.text()).toBeTruthy()

    let historyRequests = 0
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (/^\/internal\/agent\/[^/]+\/history$/.test(url.pathname)) {
        historyRequests += 1
      }
    })

    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.chat-header-name').waitFor({ state: 'visible', timeout: 30_000 })
    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const baselineHistoryRequests = historyRequests
    const incomingToken = `inactive-dm-${Date.now()}`
    await sendAsUser(request, agentName, `#${channelName}`, incomingToken)

    const channelRow = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(channelRow.locator('.sidebar-unread-badge')).toHaveText('1')
    expect(historyRequests).toBe(baselineHistoryRequests)

    await channelRow.locator('.sidebar-channel-button').click()
    await expect(page.locator('.message-item').filter({ hasText: incomingToken }).first()).toBeVisible()
  })
})
