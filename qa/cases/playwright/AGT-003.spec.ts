import { test, expect } from '@playwright/test'
import {
  createAgentApi,
  deleteAgentApi,
  listAgents,
  sendAsUser,
  getWhoami,
  historyForUser,
} from './helpers/api'
import { openAgentChat } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — AGT-003 Agent Delete And Name-Reuse Contract
 */
test.describe('AGT-003', () => {
  test('Agent Delete And Name-Reuse Contract @case AGT-003', async ({ page, request }) => {
    const name = `qa-delete-agent-${Date.now()}`
    const { username } = await getWhoami(request)
    await createAgentApi(request, { name, runtime: 'claude', model: 'sonnet' })
    await sendAsUser(request, username, `dm:@${name}`, `History seed ${name}`)
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–6: Delete agent and verify it disappears', async () => {
      await openAgentChat(page, name)
      await deleteAgentApi(request, name, 'preserve_workspace')
      await page.reload({ waitUntil: 'networkidle' })
      await expect(page.locator('.sidebar-item').filter({ hasText: name })).toHaveCount(0)
      const agents = await listAgents(request)
      expect(agents.some((agent) => agent.name === name)).toBe(false)
    })

    await test.step('Steps 7–8: Recreate same name without inheriting stale live state', async () => {
      await createAgentApi(request, { name, runtime: 'codex', model: 'gpt-5.4-mini' })
      await page.reload({ waitUntil: 'networkidle' })
      await openAgentChat(page, name)
      await expect(page.locator('.chat-header-name')).toContainText(`@${name}`)
      const dmHistory = await historyForUser(request, username, `dm:@${name}`, 30)
      expect(dmHistory.some((m) => m.senderDeleted)).toBe(false)
    })
  })
})
