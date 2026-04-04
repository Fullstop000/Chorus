import { test, expect } from './helpers/fixtures'
import {
  agentNames,
  createTeamApi,
  ensureMixedRuntimeTrio,
  ensureStubTrio,
  getWhoami,
  historyForUser,
  stopAgentApi,
  teamExists,
  waitForAgentStatus,
} from './helpers/api'
import { clickSidebarChannel, openThreadFromMessage, sendChatMessage, sendThreadMessage , gotoApp } from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const skipRealLLM = skipLLM || useStub
const agents = agentNames()
const runtimeMatrix = [
  { agentName: agents.a, runtimeLabel: 'claude', channelName: 'qa-thread-wake-claude' },
  { agentName: agents.c, runtimeLabel: 'codex', channelName: 'qa-thread-wake-codex' },
]

/**
 * Catalog: `qa/cases/teams.md` — TMT-009 Agent Team Thread Wake And In-Thread Reply
 *
 * Preconditions:
 * - mixed-runtime trio exists with at least one Claude agent and one Codex agent
 * - per-runtime team channels exist with the selected agent as a member and the current human user as an observer
 *
 * Steps:
 * 1. For each runtime agent, seed a unique parent message in its team channel.
 * 2. Stop that agent.
 * 3. Open the matching team channel.
 * 4. Open a thread from that parent message.
 * 5. Ask the stopped agent for an exact-token reply in the thread.
 * 6. Wait for wake + reply.
 * 7. Verify the reply stays in the thread.
 * 8. Verify the top-level channel does not show the thread-only reply.
 * 9. Verify the agent status is `active`.
 */
test.describe('TMT-009', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
    const { username } = await getWhoami(request)
    for (const scenario of runtimeMatrix) {
      if (await teamExists(request, scenario.channelName)) continue
      await createTeamApi(request, {
        name: scenario.channelName,
        display_name: `QA Thread Wake ${scenario.runtimeLabel}`,
        collaboration_model: 'leader_operators',
        leader_agent_name: scenario.agentName,
        members: [
          {
            member_name: scenario.agentName,
            member_type: 'agent',
            member_id: scenario.agentName,
            role: 'leader',
          },
          { member_name: username, member_type: 'human', member_id: username, role: 'observer' },
        ],
      })
    }
  })

  test('Agent Team Thread Wake And In-Thread Reply @case TMT-009', async ({ page, request }) => {
    test.skip(skipRealLLM, 'requires real LLM')
    test.setTimeout(420_000)

    const { username } = await getWhoami(request)
    await gotoApp(page)

    for (const scenario of runtimeMatrix) {
      const parentToken = `tmt9-parent-${scenario.runtimeLabel}-${Date.now()}`
      const replyToken = `tmt9-thread-${scenario.runtimeLabel}-${Date.now()}`

      await test.step(
        `Steps 1–5 (${scenario.runtimeLabel}): Seed ${scenario.agentName} parent, stop it, open team thread, and send thread prompt`,
        async () => {
          const seededParent = await request.post(`/internal/agent/${scenario.agentName}/send`, {
            data: {
              target: `#${scenario.channelName}`,
              content: `Parent marker ${parentToken}`,
            },
          })
          expect(seededParent.ok(), await seededParent.text()).toBeTruthy()

          await stopAgentApi(request, scenario.agentName)
          await clickSidebarChannel(page, scenario.channelName)
          await openThreadFromMessage(page, parentToken)
          await sendThreadMessage(
            page,
            `@${scenario.agentName} reply in this thread with exact token ${replyToken}`
          )
        }
      )

      await test.step(
        `Steps 6–9 (${scenario.runtimeLabel}): Wake ${scenario.agentName}, verify reply remains in thread, and top-level stays clean`,
        async () => {
          await waitForAgentStatus(request, scenario.agentName, 'active', 120_000)

          const deadline = Date.now() + 120_000
          let parentId: string | undefined
          let sawThreadReply = false

          while (Date.now() < deadline) {
            const topLevelHistory = await historyForUser(request, username, `#${scenario.channelName}`, 50)
            const parentMessage = topLevelHistory.find((message) =>
              (message.content ?? '').includes(parentToken)
            )
            parentId = parentMessage?.id

            if (parentId) {
              const threadHistory = await historyForUser(
                request,
                username,
                `#${scenario.channelName}:${parentId.slice(0, 8)}`,
                50
              )
              sawThreadReply = threadHistory.some(
                (message) =>
                  message.senderName === scenario.agentName &&
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
    }
  })
})
