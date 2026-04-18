import { test, expect } from './helpers/fixtures'
import {
  createTeamApi,
  ensureMixedRuntimeTrio,
  getWhoami,
  historyForUser,
  stopAgentApi,
  teamExists,
  waitForAgentStatus,
  type TrioNames,
} from './helpers/api'
import { clickSidebarChannel, openThreadFromMessage, sendChatMessage, sendThreadMessage , gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-009 Agent Team Thread Wake And In-Thread Reply
 *
 * Uses bot-b (kimi) as the only runtime that reliably responds when woken.
 * Tests thread-wake behavior with a single agent scenario.
 */
let trio: TrioNames

test.describe('TMT-009', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
    const channelName = 'qa-thread-wake-kimi'
    const { username } = await getWhoami(request)
    if (!(await teamExists(request, channelName))) {
      await createTeamApi(request, {
        name: channelName,
        display_name: 'QA Thread Wake kimi',
        collaboration_model: 'leader_operators',
        leader_agent_name: trio.botB,
        members: [
          {
            member_name: trio.botB,
            member_type: 'agent',
            member_id: trio.botB,
            role: 'leader',
          },
          { member_name: username, member_type: 'human', member_id: username, role: 'observer' },
        ],
      })
    }
  })

  test('Agent Team Thread Wake And In-Thread Reply @case TMT-009', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    const { username } = await getWhoami(request)
    const channelName = 'qa-thread-wake-kimi'
    const parentToken = `tmt9-parent-kimi-${Date.now()}`
    const replyToken = `tmt9-thread-kimi-${Date.now()}`

    await gotoApp(page)

    await test.step(
      'Steps 1–5 (kimi): Seed bot-b parent, stop it, open team thread, and send thread prompt',
      async () => {
        const seededParent = await request.post(`/internal/agent/${trio.botB}/send`, {
          data: {
            target: `#${channelName}`,
            content: `Parent marker ${parentToken}`,
          },
        })
        expect(seededParent.ok(), await seededParent.text()).toBeTruthy()

        await stopAgentApi(request, trio.botB)
        await clickSidebarChannel(page, channelName)
        await openThreadFromMessage(page, parentToken)
        await sendThreadMessage(
          page,
          `@${trio.botB} reply in this thread with exact token ${replyToken}`
        )
      }
    )

    await test.step(
      'Steps 6–9 (kimi): Wake bot-b, verify reply remains in thread, and top-level stays clean',
      async () => {
        await waitForAgentStatus(request, trio.botB, 'active', 120_000)

        const deadline = Date.now() + 120_000
        let parentId: string | undefined
        let sawThreadReply = false

        while (Date.now() < deadline) {
          const topLevelHistory = await historyForUser(request, username, `#${channelName}`, 50)
          const parentMessage = topLevelHistory.find((message) =>
            (message.content ?? '').includes(parentToken)
          )
          parentId = parentMessage?.id

          if (parentId) {
            const threadHistory = await historyForUser(
              request,
              username,
              `#${channelName}:${parentId.slice(0, 8)}`,
              50
            )
            sawThreadReply = threadHistory.some(
              (message) =>
                message.senderName === trio.botB &&
                (message.content ?? '').includes(replyToken)
            )
            if (sawThreadReply) {
              expect(
                topLevelHistory.some((message) => (message.content ?? '').includes(replyToken))
              ).toBe(false)
              break
            }
          }

          await new Promise((resolve) => setTimeout(resolve, 4000))
        }

        expect(parentId).toBeTruthy()
        expect(sawThreadReply).toBe(true)
        await expect(page.locator('.thread-panel')).toContainText(replyToken)
      }
    )
  })
})
