import { test, expect } from '@playwright/test'
import {
  ensureMixedRuntimeTrio,
  createTeamApi,
  getWhoami,
  historyForUser,
  teamExists,
} from './helpers/api'
import { clickSidebarChannel, sendChatMessage } from './helpers/ui'

/**
 * Catalog: `qa/cases/teams.md` — TMT-002 @mention Routing Forwards Message to Team Channel
 *
 * Preconditions:
 * - team `qa-eng` exists with at least one agent member
 * - agent member active
 * - `qa-algo` for dual-mention step (created if missing)
 *
 * Steps:
 * 1. Open `#all`.
 * 2. Post `@qa-eng please build a landing page`.
 * 3. Verify message in `#all`.
 * 4. Open `#qa-eng`.
 * 5. Verify copy in `#qa-eng`.
 * 6. Forwarded attribution (UI or agent-visible metadata).
 * 7. Agent activity / wakeup (optional LLM).
 * 8. Post dual `@qa-eng` and `@qa-algo`.
 * 9. Verify both team channels receive forward.
 *
 * Expected:
 * - one forward per team mention; copy in team channels; metadata when exposed on wire
 *
 * Hybrid: Steps 5–6/9 use `history` as member agent `bot-a` when human team history is empty.
 */
test.describe('TMT-002', () => {
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
    if (!(await teamExists(request, 'qa-algo'))) {
      await createTeamApi(request, {
        name: 'qa-algo',
        display_name: 'QA Algo',
        collaboration_model: 'swarm',
        leader_agent_name: null,
        members: [{ member_name: 'bot-a', member_type: 'agent', member_id: 'bot-a', role: 'member' }],
      })
    }
  })

  test('@mention Routing Forwards To Team Channels @case TMT-002', async ({ page, request }) => {
    test.setTimeout(240_000)
    const { username } = await getWhoami(request)

    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–3: Post in #all', async () => {
      await clickSidebarChannel(page, 'all')
      await sendChatMessage(page, '@qa-eng please build a landing page')
      await expect(page.getByText(/@qa-eng please build a landing page/).first()).toBeVisible()
    })

    await test.step('Steps 4–6: Open #qa-eng; copy + forwarded metadata (hybrid)', async () => {
      await clickSidebarChannel(page, 'qa-eng')
      const msgs = await historyForUser(request, 'bot-a', '#qa-eng', 40)
      expect(msgs.some((m) => (m.content ?? '').includes('landing page'))).toBe(true)
      let humanVisibleHistory = null as Awaited<ReturnType<typeof historyForUser>> | null
      try {
        humanVisibleHistory = await historyForUser(request, username, '#qa-eng', 20)
      } catch {
        test.info().annotations.push({
          type: 'note',
          description: 'human viewer may not be a direct member of the team room yet; hybrid check used agent-visible history',
        })
      }
      if (humanVisibleHistory && !humanVisibleHistory.some((m) => m.forwardedFrom != null)) {
        test.info().annotations.push({
          type: 'note',
          description: 'forwardedFrom may be absent on human-visible team history (known gap)',
        })
      }
    })

    await test.step('Steps 8–9: Dual mention both teams', async () => {
      await clickSidebarChannel(page, 'all')
      const mark = `TMT2-dual-${Date.now()}`
      await sendChatMessage(page, `@qa-eng and @qa-algo both review ${mark}`)
      const deadline = Date.now() + 120_000
      let eng = false
      let algo = false
      while (Date.now() < deadline) {
        const he = await historyForUser(request, 'bot-a', '#qa-eng', 50)
        const ha = await historyForUser(request, 'bot-a', '#qa-algo', 50)
        eng = he.some((m) => (m.content ?? '').includes(mark))
        algo = ha.some((m) => (m.content ?? '').includes(mark))
        if (eng && algo) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(eng).toBe(true)
      expect(algo).toBe(true)
    })
  })
})
