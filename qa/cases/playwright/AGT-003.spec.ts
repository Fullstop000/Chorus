import { test, expect } from './helpers/fixtures'
import {
  createAgentApi,
  deleteAgentApi,
  listAgents,
  sendAsUser,
  getWhoami,
  historyForUser,
} from './helpers/api'
import { openAgentChat , gotoApp , reloadApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — AGT-003 Agent Delete And Name-Reuse Contract
 */
test.describe('AGT-003', () => {
  test('Agent Delete And Name-Reuse Contract @case AGT-003', async ({ page, request }) => {
    const name = `qa-delete-agent-${Date.now()}`
    const { username } = await getWhoami(request)
    const { name: agentName } = await createAgentApi(request, { name, runtime: 'codex', model: 'gpt-5.4-mini' })
    await sendAsUser(request, username, `dm:@${agentName}`, `History seed ${agentName}`)
    await gotoApp(page)

    await test.step('Steps 1–6: Delete agent and verify it disappears', async () => {
      await openAgentChat(page, name)
      await deleteAgentApi(request, agentName, 'preserve_workspace')
      await reloadApp(page)
      await expect(page.locator('.sidebar-item').filter({ hasText: name })).toHaveCount(0)
      const agents = await listAgents(request)
      expect(agents.some((agent) => agent.name === agentName)).toBe(false)
    })

    await test.step('Steps 7–8: Recreate same name without inheriting stale live state', async () => {
      const { name: reusedName } = await createAgentApi(request, { name, runtime: 'codex', model: 'gpt-5.4-mini' })
      await reloadApp(page)
      await openAgentChat(page, name)
      await expect(page.locator('.chat-header-name')).toContainText(`@${name}`)
      const dmHistory = await historyForUser(request, username, `dm:@${reusedName}`, 30)
      expect(dmHistory.some((m) => m.senderDeleted)).toBe(false)
    })
  })
})
