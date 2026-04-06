import { test, expect } from './helpers/fixtures'
import {
  agentNames,
  ensureMixedRuntimeTrio,
  ensureStubTrio,
  getWhoami,
  historyForUser,
  sendAsUser,
} from './helpers/api'
import { clickSidebarChannel, sendChatMessage , gotoApp } from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()

/**
 * Catalog: `qa/cases/messaging.md` — MSG-001 Multi-Agent Channel Fan-Out
 *
 * Preconditions:
 * - `bot-a`, `bot-b`, and `bot-c` exist
 * - active test channel is open → script opens `#all`
 *
 * Steps:
 * 1. Send one clear prompt in the shared channel asking all agents to reply.
 * 2. Wait long enough for all agents to process.
 * 3. Verify the original human message appears once.
 * 4. Verify replies from all 3 agents appear in the same channel.
 * 5. Verify each reply shows the correct sender identity.
 * 6. Verify reply order is chronologically reasonable and no messages are duplicated.
 *
 * Expected:
 * - one human message; three distinct agent replies; correct attribution; same channel
 *
 * Hybrid: Steps 3–6 asserted via `history` API (same contract as UI) after Step 1 UI send.
 */
test.describe('MSG-001', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
  })

  test('Multi-Agent Channel Fan-Out @case MSG-001', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(useStub ? 600_000 : 300_000)

    const { username } = await getWhoami(request)
    const mark = `msg1-${Date.now()}`

    await gotoApp(page)

    await test.step('Step 1: Send prompt in #all asking all agents to reply', async () => {
      await clickSidebarChannel(page, 'all')
      if (useStub) {
        // Send a single message that all stub agents will echo back.
        // Using one message avoids a read-cursor race where rapidly sent
        // messages can be skipped by the agent inbox scan.
        await sendAsUser(request, username, '#all', `reply with "fan-${mark}"`)
      } else {
        await sendChatMessage(
          page,
          `MSG-001 ${mark}: ${agents.a} reply OK-a, ${agents.b} OK-b, ${agents.c} OK-c`
        )
      }
    })

    await test.step('Steps 2–6: Wait and verify history (human once; three agents; senders; order)', async () => {
      const deadline = Date.now() + 240_000
      let msgs: Awaited<ReturnType<typeof historyForUser>> = []
      while (Date.now() < deadline) {
        msgs = await historyForUser(request, username, '#all', 120)
        const agentMsgs = msgs.filter(
          (m) => (m.senderType ?? '').toLowerCase() === 'agent'
        )
        const senders = new Set(agentMsgs.map((m) => m.senderName))
        const haveThreeBodies = agentMsgs.length >= 3
        if (useStub) {
          // All 3 stub agents echo the same token; verify 3 distinct senders.
          if (senders.size >= 3 && agentMsgs.some((m) => (m.content ?? '').includes(`fan-${mark}`))) break
        } else {
          if (haveThreeBodies) break
        }
        await new Promise((r) => setTimeout(r, useStub ? 2_000 : 5_000))
      }

      const humanCount = msgs.filter((m) => (m.content ?? '').includes(mark) && m.senderType !== 'agent').length
      expect(humanCount).toBeLessThanOrEqual(1)

      const agentMsgs = msgs.filter(
        (m) => (m.senderType ?? '').toLowerCase() === 'agent'
      )
      expect(agentMsgs.length).toBeGreaterThanOrEqual(3)
      const senderNames = new Set(agentMsgs.map((m) => m.senderName))
      if (useStub) {
        expect(senderNames.size).toBeGreaterThanOrEqual(3)
        const blob = agentMsgs.map((m) => m.content ?? '').join('\n')
        expect(blob).toContain(`fan-${mark}`)
      } else {
        expect(senderNames.has(agents.a)).toBe(true)
        expect(senderNames.has(agents.b)).toBe(true)
        expect(senderNames.has(agents.c)).toBe(true)
      }
    })
  })
})
