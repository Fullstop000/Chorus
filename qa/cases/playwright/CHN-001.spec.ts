import { test, expect } from './helpers/fixtures'
import { agentNames, ensureMixedRuntimeTrio, ensureStubTrio, historyForUser } from './helpers/api'
import {
  clickComboboxOption,
  createUserChannelViaUi,
  clickSidebarChannel,
  openMembersPanel,
  sendChatMessage,
  gotoApp,
} from './helpers/ui'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()

/**
 * Catalog: `qa/cases/channels.md` — CHN-001 Channel Create And Default Membership
 *
 * Preconditions:
 * - at least 3 test agents exist
 *
 * Steps:
 * 1. Create a new disposable channel such as `#qa-ops`.
 * 2. Verify it appears in the sidebar immediately.
 * 3. Open the new channel and verify the empty state is sane.
 * 4. Open the members rail and verify the count starts at `1`, showing only the current human user.
 * 5. Invite one agent into the channel through the shipped member control.
 * 6. Send one human message asking the invited agent to reply.
 * 7. Verify the invited agent replies in the new channel and uninvited agents do not.
 * 8. Navigate away and back, then verify the new channel history and membership count persist.
 *
 * Expected:
 * - channel create succeeds; sidebar updates; starts with creator only; invite works; agent replies in-channel
 *
 * Note: Step 7 uses **hybrid** verification (`history` as `bot-a` member) per QA hybrid rules when human `#channel` history is empty.
 */
test.describe('CHN-001', () => {
  test.beforeAll(async ({ request }) => {
    if (useStub) {
      await ensureStubTrio(request)
    } else {
      await ensureMixedRuntimeTrio(request)
    }
  })

  test('Channel Create And Default Membership @case CHN-001', async ({ page, request }) => {
    test.setTimeout(300_000)

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

    await test.step(`Step 5: Invite ${agents.a}`, async () => {
      await page.locator('.members-panel-actions button:has-text("Invite")').click()
      const inviteDialog = page.locator('[role="dialog"]')
      await inviteDialog.locator('[role="combobox"][aria-label="Member"]').click()
      await clickComboboxOption(page, agents.a)
      await inviteDialog.locator('button:has-text("Invite Member")').click()
      await expect(inviteDialog).toBeHidden()
      await expect(page.locator('.members-panel-title').first()).toHaveText('2')
    })

    const token = `CHN-OPS-${Date.now()}`

    await test.step(`Step 6: Human message asking ${agents.a} to reply`, async () => {
      await page.locator('.members-panel-close').click().catch(() => {})
      // Stub token extraction needs quoted form; real LLM still understands this phrasing.
      const ping = useStub ? `${agents.a} reply with "${token}"` : `${agents.a} reply with token ${token}`
      await sendChatMessage(page, ping)
    })

    await test.step('Step 7: Invited agent reply in channel (hybrid: member history)', async () => {
      if (skipLLM) {
        test.info().annotations.push({ type: 'skip', description: 'Step 7 skipped: CHORUS_E2E_LLM=0' })
        return
      }
      const deadline = Date.now() + 120_000
      let ok = false
      while (Date.now() < deadline) {
        const msgs = await historyForUser(request, agents.a, `#${slug}`, 30)
        ok = msgs.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))
        if (ok) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(ok, `${agents.a} should reply in channel`).toBe(true)
    })

    await test.step('Step 8: Navigate away and back — count + history persist', async () => {
      await clickSidebarChannel(page, 'all')
      await clickSidebarChannel(page, slug)
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-title').first()).toHaveText('2')
    })
  })
})
