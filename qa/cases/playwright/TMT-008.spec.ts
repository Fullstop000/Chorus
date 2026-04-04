import { test, expect } from './helpers/fixtures'
import {
  agentNames,
  ensureMixedRuntimeTrio,
  ensureStubTrio,
  createTeamApi,
  getWhoami,
  historyForUser,
  sendAsUser,
  teamExists,
} from './helpers/api'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const skipRealLLM = skipLLM || useStub
const agents = agentNames()

/**
 * Catalog: `qa/cases/teams.md` — TMT-008 Multi-Team Agent Context Isolation
 *
 * Preconditions:
 * - `qa-eng` Leader+Operators, bot-a leader
 * - `qa-algo` Swarm, bot-a member
 *
 * Steps:
 * 1. Ask bot-a in `#all` for teams + roles.
 * 2. Verify names both teams + roles.
 * 3–4. `@qa-eng` vs `@qa-algo` — no deliberation vs deliberation (hybrid history).
 * 5–6. Role-appropriate behavior (LLM / manual depth).
 *
 * Expected:
 * - agent reports both memberships; models do not bleed across channels
 */
test.describe('TMT-008', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
    if (!(await teamExists(request, 'qa-eng'))) {
      await createTeamApi(request, {
        name: 'qa-eng',
        display_name: 'QA Engineering',
        collaboration_model: 'leader_operators',
        leader_agent_name: agents.a,
        members: [
          { member_name: agents.a, member_type: 'agent', member_id: agents.a, role: 'operator' },
          { member_name: agents.b, member_type: 'agent', member_id: agents.b, role: 'operator' },
        ],
      })
    }
    if (!(await teamExists(request, 'qa-algo'))) {
      await createTeamApi(request, {
        name: 'qa-algo',
        display_name: 'QA Algo',
        collaboration_model: 'swarm',
        leader_agent_name: null,
        members: [{ member_name: agents.a, member_type: 'agent', member_id: agents.a, role: 'member' }],
      })
    }
  })

  test('Multi-team context @case TMT-008', async ({ request }) => {
    test.skip(skipRealLLM, 'requires real LLM')
    test.setTimeout(360_000)

    const { username } = await getWhoami(request)

    await test.step(`Steps 1–2: ${agents.a} lists qa-eng and qa-algo`, async () => {
      const mark = `tmt8-${Date.now()}`
      await sendAsUser(
        request,
        username,
        '#all',
        `${agents.a} ${mark}: what teams are you in and your role in each? mention qa-eng and qa-algo.`
      )
      const deadline = Date.now() + 180_000
      let text = ''
      while (Date.now() < deadline) {
        const msgs = await historyForUser(request, username, '#all', 40)
        const fromA = msgs.filter((m) => m.senderName === agents.a && (m.content ?? '').includes(mark))
        if (fromA.length) {
          text = fromA[fromA.length - 1].content ?? ''
          break
        }
        await new Promise((r) => setTimeout(r, 5000))
      }
      expect(text.toLowerCase()).toMatch(/qa-eng/)
      expect(text.toLowerCase()).toMatch(/qa-algo/)
    })

    await test.step('Steps 3–4 (hybrid): #qa-eng no swarm system line; #qa-algo may show deliberation', async () => {
      await sendAsUser(request, username, '#all', '@qa-eng design a minimal API ping')
      await new Promise((r) => setTimeout(r, 25_000))
      const engMsgs = await historyForUser(request, agents.a, '#qa-eng', 30)
      const engDelib = engMsgs.some(
        (m) =>
          (m.senderType === 'system' || m.senderName === 'system') &&
          (m.content ?? '').includes('Discuss the best approach')
      )
      expect(engDelib).toBe(false)

      await sendAsUser(request, username, '#all', '@qa-algo analyze results briefly')
      await new Promise((r) => setTimeout(r, 40_000))
      const algoMsgs = await historyForUser(request, agents.a, '#qa-algo', 40)
      const algoPrompt = algoMsgs.some(
        (m) =>
          (m.senderType === 'system' || m.senderName === 'system') &&
          (m.content ?? '').includes('New task received')
      )
      expect(algoPrompt).toBe(true)
    })
  })
})
