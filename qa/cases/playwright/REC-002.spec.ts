import { test, expect } from './helpers/fixtures'
import { agentNames, ensureMixedRuntimeTrio, ensureStubTrio, getWhoami, historyForUser } from './helpers/api'
import { clickSidebarChannel, openAgentTab, openThreadFromMessage, sendChatMessage , gotoApp } from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()

/**
 * Catalog: `qa/cases/agents.md` — REC-002 Concurrent Agent Activity Under One Channel
 */
test.describe('REC-002', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
  })

  test('Concurrent Agent Activity Under One Channel @case REC-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    const { username } = await getWhoami(request)
    const mark = `rec-002-${Date.now()}`
    await gotoApp(page)

    await test.step('Steps 1–4: Trigger multi-agent replies, switch activity, and open a thread', async () => {
      await clickSidebarChannel(page, 'all')
      await sendChatMessage(page, `MSG ${mark}: ${agents.a} say a-${mark}, ${agents.b} say b-${mark}, ${agents.c} say c-${mark}`)
      await openAgentTab(page, agents.a, 'Activity')
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      const deadline = Date.now() + 180_000
      let sawAll = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, '#all', 80)
        const text = history.map((m) => m.content ?? '').join(' ')
        sawAll = /a-/.test(text) && /b-/.test(text) && /c-/.test(text)
        if (sawAll) break
        await new Promise((r) => setTimeout(r, 5000))
      }
      expect(sawAll).toBe(true)
      await openThreadFromMessage(page, mark)
      await expect(page.locator('.thread-panel')).toBeVisible()
      await page.locator('.thread-close-btn').click()
      await expect(page.locator('.message-item').filter({ hasText: mark }).first()).toBeVisible()
    })
  })
})
