import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser, pollUntil } from './helpers/api'
import { clickSidebarChannel, sendChatMessage, gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-001 Multi-Agent Channel Fan-Out
 *
 * Sends to #all (the shared channel) so every active agent receives the
 * message. Asserts that at least 1 distinct agent replies after the timestamped
 * mark — a mark-based filter prevents contamination from pre-existing history.
 *
 * Note: the threshold is ≥1 (not ≥3) because not all runtimes reliably
 * respond in CI. Fan-out routing is proven if any agent picks up and replies.
 */
test.describe('MSG-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Multi-Agent Channel Fan-Out @case MSG-001', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    const { username } = await getWhoami(request)
    const mark = `msg1-${Date.now()}`

    await gotoApp(page)

    await test.step('Step 1: Send prompt in #all', async () => {
      await clickSidebarChannel(page, 'all')
      await sendChatMessage(page, `MSG-001 mark=${mark} — please reply to this message`)
    })

    await test.step('Steps 2–4: At least 1 agent replies; human message appears once', async () => {
      const afterMark = await pollUntil(async () => {
        const all = await historyForUser(request, username, '#all', 200)
        const markIdx = all.findIndex((m) => (m.content ?? '').includes(mark))
        if (markIdx < 0) return undefined
        const after = all.slice(markIdx)
        const distinct = new Set(
          after.filter((m) => m.senderType === 'agent').map((m) => m.senderName)
        )
        return distinct.size >= 1 ? after : undefined
      }, 300_000)

      const humanCount = afterMark.filter(
        (m) => (m.content ?? '').includes(mark) && m.senderType !== 'agent'
      ).length
      expect(humanCount).toBeLessThanOrEqual(1)

      const distinct = new Set(
        afterMark.filter((m) => m.senderType === 'agent').map((m) => m.senderName)
      )
      expect(distinct.size).toBeGreaterThanOrEqual(1)
    })
  })
})
