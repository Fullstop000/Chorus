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
 * Catalog: `qa/cases/teams.md` — TMT-004 Swarm Collaboration Model with Deliberation Phase
 *
 * Preconditions:
 * - team `qa-swarm` swarm with bot-a and bot-b
 *
 * Steps:
 * 1. `@qa-swarm research...` from `#all`
 * 2. Open `#qa-swarm`
 * 3. System line "New task received" + READY instructions
 * 4–7. Agent discussion, READY, GO, execute (LLM)
 * 8–9. Second task queue (not fully automated)
 *
 * Expected:
 * - deliberation prompt after forward; quorum behavior (manual depth)
 *
 * This script asserts Step 3 signal when LLM enabled. Steps 4–9 require longer manual observation.
 */
test.describe('TMT-004', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
    if (!(await teamExists(request, 'qa-swarm'))) {
      await createTeamApi(request, {
        name: 'qa-swarm',
        display_name: 'QA Swarm',
        collaboration_model: 'swarm',
        leader_agent_name: null,
        members: [
          { member_name: agents.a, member_type: 'agent', member_id: agents.a, role: 'member' },
          { member_name: agents.b, member_type: 'agent', member_id: agents.b, role: 'member' },
        ],
      })
    }
  })

  test('Swarm deliberation system line @case TMT-004', async ({ request }) => {
    test.skip(skipRealLLM, 'requires real LLM')
    test.setTimeout(300_000)

    const { username } = await getWhoami(request)
    const mark = `tmt4-${Date.now()}`

    await test.step('Step 1: Forward task to qa-swarm', async () => {
      await sendAsUser(request, username, '#all', `@qa-swarm research ${mark} best frontend framework`)
    })

    await test.step('Steps 2–3: System deliberation prompt in #qa-swarm', async () => {
      const deadline = Date.now() + 120_000
      let ok = false
      while (Date.now() < deadline) {
        const msgs = await historyForUser(request, agents.a, '#qa-swarm', 50)
        ok = msgs.some(
          (m) =>
            (m.senderType === 'system' || m.senderName === 'system') &&
            (m.content ?? '').includes('New task received')
        )
        if (ok) break
        await new Promise((r) => setTimeout(r, 5000))
      }
      expect(ok).toBe(true)
    })
  })
})
