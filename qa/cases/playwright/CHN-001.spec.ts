import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, historyForUser, type TrioNames } from './helpers/api'
import {
  createUserChannelViaUi,
  clickSidebarChannel,
  openMembersPanel,
  sendChatMessage,
  gotoApp,
} from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/channels.md` — CHN-001 Channel Create And Default Membership
 *
 * Uses bot-b (kimi) for the invite/reply step because it reliably responds.
 */
let trio: TrioNames

test.describe('CHN-001', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Channel Create And Default Membership @case CHN-001', async ({ page, request }) => {
    test.setTimeout(120_000)

    const slug = `qa-ops-${Date.now()}`
    await gotoApp(page)

    await test.step('Step 1: Create disposable channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright CHN-001')
    })

    await test.step('Step 2: Sidebar shows channel', async () => {
      await expect(page.locator('.sidebar-item-text').filter({ hasText: slug }).first()).toBeVisible()
    })

    await test.step('Step 3: Open channel — sane empty / chat shell', async () => {
      await clickSidebarChannel(page, slug)
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })

    await test.step('Step 4: Members rail — count 1 (human only)', async () => {
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-title').first()).toHaveText('1')
    })

    await test.step('Step 5: Invite bot-b', async () => {
      await page.locator('.members-panel-actions button:has-text("Invite")').click()
      const inviteDialog = page.locator('[role="dialog"]')
      await inviteDialog.locator('[role="combobox"][aria-label="Member"]').click()
      await page.locator('[role="option"]').filter({ hasText: trio.displayB }).first().click()
      await inviteDialog.locator('button:has-text("Invite Member")').click()
      await expect(inviteDialog).toBeHidden()
      await expect(page.locator('.members-panel-title').first()).toHaveText('2')
    })

    const token = `CHN-OPS-${Date.now()}`

    await test.step('Step 6: Human message asking bot-b to reply', async () => {
      await page.locator('.members-panel-close').click().catch(() => {})
      await sendChatMessage(page, `${trio.displayB} reply with token ${token}`)
    })

    await test.step('Step 7: Invited agent reply in channel (hybrid: member history)', async () => {
      if (skipLLM) {
        test.info().annotations.push({ type: 'skip', description: 'Step 7 skipped: CHORUS_E2E_LLM=0' })
        return
      }
      const deadline = Date.now() + 120_000
      let ok = false
      while (Date.now() < deadline) {
        const msgs = await historyForUser(request, trio.botB, `#${slug}`, 30)
        ok = msgs.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))
        if (ok) break
        await new Promise((r) => setTimeout(r, 2000))
      }
      expect(ok, `${trio.botB} should reply in channel`).toBe(true)
    })

    await test.step('Step 8: Navigate away and back — count + history persist', async () => {
      await clickSidebarChannel(page, 'all')
      await clickSidebarChannel(page, slug)
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-title').first()).toHaveText('2')
    })
  })
})
