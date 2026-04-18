import { test, expect } from './helpers/fixtures'
import {
  createChannelApi,
  ensureMixedRuntimeTrio,
  getChannelMembersApi,
  getWhoami,
} from './helpers/api'
import { clickSidebarChannel, openMembersPanel, closeMembersPanel, gotoApp , reloadApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/channels.md` — CHN-003 Channel Invite Operations And `#all` Guardrails
 */
test.describe('CHN-003', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Channel Invite Operations And #all Guardrails @case CHN-003', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    const channel = await createChannelApi(request, {
      name: `qa-members-${Date.now()}`,
      description: 'playwright CHN-003',
    })
    await gotoApp(page)

    await test.step('Steps 1–4: #all has no invite button and shows complete membership', async () => {
      await clickSidebarChannel(page, 'all')
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-actions button:has-text("Invite")')).toHaveCount(0)
      const serverInfoRes = await request.get('/api/server-info')
      expect(serverInfoRes.ok(), await serverInfoRes.text()).toBeTruthy()
      const serverInfo = await serverInfoRes.json()
      const allId = serverInfo.system_channels.find((channel: { name: string; id?: string }) => channel.name === 'all')?.id
      expect(allId).toBeTruthy()
      const members = await getChannelMembersApi(request, allId)
      const names = members.members.map((m) => m.memberName)
      expect(names).toContain(username)
      expect(names).toContain('bot-a')
      expect(names).toContain('bot-b')
      expect(names).toContain('bot-c')
      await closeMembersPanel(page)
    })

    await test.step('Steps 5–8: User channel invite updates and persists', async () => {
      await clickSidebarChannel(page, channel.name)
      await openMembersPanel(page)
      await page.locator('.members-panel-actions button:has-text("Invite")').click()
      const dialog = page.locator('[role="dialog"]')
      await dialog.locator('[role="combobox"][aria-label="Member"]').click()
      await page.locator('[role="option"]').filter({ hasText: 'bot-a' }).first().click()
      await dialog.locator('button:has-text("Invite Member")').click()
      await expect(page.locator('.members-panel-title')).toHaveText('2')
      const after = await getChannelMembersApi(request, channel.id)
      expect(after.members.map((m) => m.memberName)).toContain('bot-a')
      await reloadApp(page)
      await clickSidebarChannel(page, channel.name)
      await openMembersPanel(page)
      await expect(page.locator('.members-panel-title')).toHaveText('2')
    })
  })
})
