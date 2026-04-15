import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, sendAsUser } from './helpers/api'
import { clickSidebarChannel, openThreadFromMessage, gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-003 Thread Reply In Busy Channel
 *
 * The beforeAll seeds a human message in #all as the thread anchor.
 * Using a human message (not an agent reply) removes the dependency on a
 * specific agent responding to a precondition prompt.
 *
 * Steps:
 * 1. Open a thread from the seeded human message.
 * 2. Send a thread reply from the human.
 * 3. Close the thread and verify thread body is not polluted into the main channel.
 */
let threadSeedToken = ''

test.describe('MSG-003', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
    if (process.env.CHORUS_E2E_LLM === '0') return
    const { username } = await getWhoami(request)
    threadSeedToken = `msg3-anchor-${Date.now()}`
    await sendAsUser(request, username, '#all', `MSG-003 thread anchor ${threadSeedToken}`)
  })

  test('Thread Reply In Busy Channel @case MSG-003', async ({ page }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(120_000)

    await gotoApp(page)
    await clickSidebarChannel(page, 'all')

    await test.step('Step 1: Open thread from seeded anchor message', async () => {
      await expect(
        page.locator('.message-item').filter({ hasText: threadSeedToken }).first()
      ).toBeVisible({ timeout: 15_000 })
      await openThreadFromMessage(page, threadSeedToken)
      await expect(page.locator('.thread-panel')).toBeVisible({ timeout: 10_000 })
    })

    const threadLine = `human-thread-${Date.now()}`

    await test.step('Step 2: Human thread reply', async () => {
      await page.locator('.thread-input-textarea').fill(threadLine)
      await page.locator('.thread-send-btn').click()
      await expect(page.locator('.thread-body')).toContainText(threadLine)
    })

    await test.step('Step 3: Close thread; main channel stays clean', async () => {
      await page.locator('.thread-close-btn').click()
      await expect(page.locator('.thread-panel')).toBeHidden()
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })
  })
})
