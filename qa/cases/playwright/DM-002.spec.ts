import { test, expect } from './helpers/fixtures'
import {
  createAgentApi,
  getAgentActivityLogApi,
  getWhoami,
  historyForUser,
  waitForAgentActive,
} from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage, gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/messaging.md` — DM-002 Single-Agent DM E2E Reply
 *
 * Runtime-parameterisable: set CHORUS_RUNTIME and CHORUS_MODEL to target any
 * supported runtime with the same test routine.
 *
 * Defaults: CHORUS_RUNTIME=claude, CHORUS_MODEL=sonnet
 *
 * Preconditions:
 * - server running
 * - agent `dm-e2e-<runtime>` seeded by beforeAll
 *
 * Steps:
 * 1. Seed agent dm-e2e-<runtime> with the chosen runtime/model.
 * 2. Open a DM with the agent.
 * 3. Send a message with a unique exact token.
 * 4. Verify the human message appears immediately in the DM timeline.
 * 5. Poll history API for agent reply containing the token.
 * 6. Verify reply appears in the DM in the browser, not in a channel.
 * 7. Verify activity log shows a send_message tool_start entry.
 *
 * Expected:
 * - human message visible immediately
 * - agent reply in same DM with token
 * - activity log confirms send_message tool call (not raw stdout)
 */

const skipLLM = process.env.CHORUS_E2E_LLM === '0'
const runtime = process.env.CHORUS_RUNTIME ?? 'claude'
const model = process.env.CHORUS_MODEL ?? 'sonnet'
const agentName = `dm-e2e-${runtime}`

test.describe('DM-002', () => {
  test.beforeAll(async ({ request }) => {
    try {
      await createAgentApi(request, { name: agentName, runtime, model })
    } catch (_e) {
      // agent already exists
    }
  })

  test(`Single-Agent DM E2E Reply [${runtime}/${model}] @case DM-002`, async ({
    page,
    request,
  }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(180_000)

    const { username } = await getWhoami(request)
    const token = `DM2-${Date.now()}`
    const prompt = `Reply only in this DM. Return the exact token: ${token}`

    await waitForAgentActive(request, agentName)
    await gotoApp(page)

    await test.step('Step 1-2: Open DM with agent', async () => {
      await openAgentChat(page, agentName)
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })

    await test.step('Step 3-4: Send message and verify human row visible', async () => {
      await sendChatMessage(page, prompt)
      await expect(page.getByText(prompt).first()).toBeVisible({ timeout: 10_000 })
    })

    let replyMsgs: Awaited<ReturnType<typeof historyForUser>> = []

    await test.step('Step 5: Poll history API for agent reply with token', async () => {
      const deadline = Date.now() + 150_000
      while (Date.now() < deadline) {
        replyMsgs = await historyForUser(request, username, `dm:@${agentName}`, 40)
        if (
          replyMsgs.some(
            (m) =>
              m.senderType === 'agent' &&
              m.senderName === agentName &&
              (m.content ?? '').includes(token)
          )
        ) {
          break
        }
        await new Promise((r) => setTimeout(r, 4_000))
      }
      const replies = replyMsgs.filter(
        (m) =>
          m.senderType === 'agent' &&
          m.senderName === agentName &&
          (m.content ?? '').includes(token)
      )
      expect(replies.length).toBeGreaterThanOrEqual(1)
      expect(replies.map((r) => r.content ?? '').join(' ')).toContain(token)
    })

    await test.step('Step 6: Reply visible in DM timeline in browser', async () => {
      await expect(
        page.locator('.message-item').filter({ hasText: token }).first()
      ).toBeVisible({ timeout: 15_000 })
    })

    await test.step('Step 7: Activity log shows send_message tool call', async () => {
      await openAgentTab(page, agentName, 'Activity')
      const deadline = Date.now() + 30_000
      // v2 drivers log raw tool names: mcp__chat__send_message (claude),
      // send_message (kimi/stub), chat_send_message (opencode)
      const isSendTool = (name: string) =>
        name.includes('send_message') || name.includes('Sending message')
      let activity = await getAgentActivityLogApi(request, agentName)
      while (
        Date.now() < deadline &&
        !activity.entries.some(
          (item) => item.entry.kind === 'tool_call' && isSendTool(item.entry.tool_name ?? '')
        )
      ) {
        await new Promise((r) => setTimeout(r, 2_000))
        activity = await getAgentActivityLogApi(request, agentName)
      }

      const sentTool = activity.entries.some(
        (item) => item.entry.kind === 'tool_call' && isSendTool(item.entry.tool_name ?? '')
      )
      expect(sentTool).toBe(true)

      const rawTextOnlyReply = activity.entries.some(
        (item) => item.entry.kind === 'text' && (item.entry.text ?? '').includes(token)
      )
      expect(rawTextOnlyReply).toBe(false)
    })
  })
})
