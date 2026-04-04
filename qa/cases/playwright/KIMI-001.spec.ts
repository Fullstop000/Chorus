import { test, expect } from './helpers/fixtures'
import {
  createAgentApi,
  getAgentActivityLogApi,
  getWhoami,
  historyForUser,
  waitForAgentActive,
} from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage , gotoApp } from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const skipRealLLM = skipLLM || useStub

/**
 * Runtime verification: direct DM to a Kimi-backed agent with an exact-token assertion.
 */
test.describe('KIMI-001', () => {
  test.beforeAll(async ({ request }) => {
    try {
      await createAgentApi(request, { name: 'bot-k', runtime: 'kimi', model: 'kimi-code/kimi-for-coding' })
    } catch (e) {
      // Ignore if already exists
    }
  })

  test('Kimi Agent Direct Reply', async ({ page, request }) => {
    test.skip(skipRealLLM, 'requires real LLM')
    test.setTimeout(300_000)

    const { username } = await getWhoami(request)
    const mark = `kimi1-${Date.now()}`
    const token = `OK-KIMI-${Date.now()}`

    await waitForAgentActive(request, 'bot-k')

    await gotoApp(page)

    await test.step('Step 1: Send direct message to bot-k asking for an exact token', async () => {
      await openAgentChat(page, 'bot-k')
      await sendChatMessage(page, `MSG-KIMI ${mark}: reply with exact token ${token}`)
    })

    await test.step('Step 2: Wait and verify the Kimi reply appears in the same DM', async () => {
      const deadline = Date.now() + 240_000
      let msgs: Awaited<ReturnType<typeof historyForUser>> = []
      while (Date.now() < deadline) {
        msgs = await historyForUser(request, username, 'dm:@bot-k', 120)
        const agents = msgs.filter(
          (m) =>
            m.senderType === 'agent' &&
            m.senderName === 'bot-k' &&
            (m.content ?? '').includes(token)
        )
        if (agents.length >= 1) break
        await new Promise((r) => setTimeout(r, 5000))
      }

      const agents = msgs.filter(
        (m) =>
          m.senderType === 'agent' &&
          m.senderName === 'bot-k' &&
          (m.content ?? '').includes(token)
      )
      expect(agents.length).toBeGreaterThanOrEqual(1)
      const bodies = agents.map((a) => a.content ?? '').join(' ')
      expect(bodies).toContain(token)
      console.log('Kimi reply:', bodies)
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
    })

    await test.step('Step 3: Verify activity shows a real send_message path, not only raw text output', async () => {
      await openAgentTab(page, 'bot-k', 'Activity')
      const deadline = Date.now() + 30_000
      let activity = await getAgentActivityLogApi(request, 'bot-k')
      while (
        Date.now() < deadline &&
        !activity.entries.some(
          (item) =>
            item.entry.kind === 'tool_start' &&
            (item.entry.tool_name ?? '').includes('Sending message')
        )
      ) {
        await new Promise((r) => setTimeout(r, 2000))
        activity = await getAgentActivityLogApi(request, 'bot-k')
      }

      const sentTool = activity.entries.some(
        (item) =>
          item.entry.kind === 'tool_start' &&
          (item.entry.tool_name ?? '').includes('Sending message')
      )
      expect(sentTool).toBe(true)

      const rawTextOnlyReply = activity.entries.some(
        (item) => item.entry.kind === 'text' && (item.entry.text ?? '').includes(token)
      )
      expect(rawTextOnlyReply).toBe(false)
    })
  })
})
