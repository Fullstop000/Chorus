import { test, expect } from './helpers/fixtures'
import { agentNames, ensureMixedRuntimeTrio, ensureStubTrio, getWhoami, historyForUser } from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage , gotoApp } from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()

/**
 * Catalog: `qa/cases/messaging.md` — MSG-004 Direct Message Wake And Reply Visibility
 */
test.describe('MSG-004', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
  })

  test('Direct Message Wake And Reply Visibility @case MSG-004', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    const { username } = await getWhoami(request)
    const token = `dm-wake-${Date.now()}`
    await request.post(`/api/agents/${agents.a}/stop`)
    await gotoApp(page)

    await test.step(`Steps 1–5: Send DM to inactive ${agents.a} and wait for wake + reply`, async () => {
      await openAgentChat(page, agents.a)
      await openAgentTab(page, agents.a, 'Profile')
      if (!useStub) {
        await expect(page.locator('.profile-config-grid')).toContainText('inactive')
      }
      await page.getByRole('button', { name: 'Chat' }).click()
      await sendChatMessage(page, `reply with "${token}"`)
      const deadline = Date.now() + 120_000
      let sawReply = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, `dm:@${agents.a}`, 40)
        sawReply = history.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))
        if (sawReply) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(sawReply).toBe(true)
    })

    await test.step(`Steps 6–9: Reply stays in same DM and lifecycle surfaces recover coherently`, async () => {
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      await openAgentTab(page, agents.a, 'Profile')
      await expect(page.locator('.profile-config-grid')).toContainText('active')
      await page.getByRole('button', { name: 'Chat' }).click()
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
    })
  })
})
