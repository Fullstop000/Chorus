import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, createTeamApi, getWhoami, historyForUser, sendAsUser, teamExists } from './helpers/api'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/teams.md` — TMT-003 Leader+Operators Collaboration Model
 *
 * Preconditions:
 * - team `qa-eng` leader_operators, leader bot-a, operator bot-b, both active
 *
 * Steps:
 * 1. Open `#all` and post `@qa-eng build a simple to-do list app`.
 * 2. Open `#qa-eng` and observe.
 * 3–6. Leader delegates / operator works / synthesis (LLM — soft assertions).
 * 7. No swarm deliberation system line in team channel.
 *
 * Expected:
 * - no deliberation prompt; leader/operator behavior (best-effort when LLM enabled)
 *
 * Automated assertion focuses on Step 7 (deterministic). Steps 3–6 are observational in manual QA.
 */
test.describe('TMT-003', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
    if (!(await teamExists(request, 'qa-eng'))) {
      await createTeamApi(request, {
        name: 'qa-eng',
        display_name: 'QA Engineering',
        collaboration_model: 'leader_operators',
        leader_agent_name: 'bot-a',
        members: [
          { member_name: 'bot-a', member_type: 'agent', member_id: 'bot-a', role: 'operator' },
          { member_name: 'bot-b', member_type: 'agent', member_id: 'bot-b', role: 'operator' },
        ],
      })
    }
  })

  test('Leader+Operators — no swarm deliberation line @case TMT-003', async ({ request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    const { username } = await getWhoami(request)

    await test.step('Step 1: @qa-eng task from #all', async () => {
      await sendAsUser(request, username, '#all', '@qa-eng build a simple to-do list app')
    })

    await test.step('Steps 2–6: Observe channel traffic (time for agents)', async () => {
      await new Promise((r) => setTimeout(r, 45_000))
    })

    await test.step('Step 7 / Expected: No swarm deliberation system message in #qa-eng', async () => {
      const msgs = await historyForUser(request, 'bot-a', '#qa-eng', 80)
      const bad = msgs.some(
        (m) =>
          (m.senderType === 'system' || m.senderName === 'system') &&
          (m.content ?? '').includes('Discuss the best approach')
      )
      expect(bad).toBe(false)
    })
  })
})
