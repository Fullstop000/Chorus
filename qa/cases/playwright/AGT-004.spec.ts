import { test, expect } from '@playwright/test'
import fs from 'node:fs/promises'
import path from 'node:path'
import {
  ensureMixedRuntimeTrio,
  getWorkspaceApi,
  restartAgentApi,
  deleteAgentApi,
  createAgentApi,
  getAgentDetail,
  getWhoami,
  sendAsUser,
  historyForUser,
} from './helpers/api'
import { openAgentTab, clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — AGT-004 Agent Control Center Edit, Restart, Delete, And Deleted History
 */
test.describe('AGT-004', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Agent Control Center Edit, Restart, Delete, And Deleted History @case AGT-004', async ({
    page,
    request,
  }) => {
    const name = `qa-profile-agent-${Date.now()}`
    const { username } = await getWhoami(request)
    await createAgentApi(request, {
      name,
      runtime: 'codex',
      model: 'gpt-5.4-mini',
      reasoningEffort: 'medium',
      description: 'initial role',
    })
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–5: Edit config and verify role/env/reasoning persist', async () => {
      await openAgentTab(page, name, 'Profile')
      await page.getByRole('button', { name: 'Edit' }).click()
      await page.locator('.modal-box-agent textarea').fill('updated role text')
      await page.locator('.modal-box-agent .modal-field:has-text("Reasoning") select').selectOption('high')
      await page.locator('.modal-box-agent .env-add-btn').click()
      const row = page.locator('.env-var-editor-row').last()
      await row.locator('input').nth(0).fill('QA_FLAG')
      await row.locator('input').nth(1).fill('on')
      await page.locator('.modal-footer button:has-text("Save")').click()
      await expect(page.locator('.profile-role-text')).toContainText('updated role text')
      await expect(page.locator('.profile-config-grid')).toContainText('high')
      await expect(page.locator('.env-var-row')).toContainText('QA_FLAG')
      const detail = await getAgentDetail(request, name)
      expect(detail.envVars.some((envVar) => envVar.key === 'QA_FLAG' && envVar.value === 'on')).toBe(true)
    })

    await test.step('Steps 6–7: Restart and reset-session restart keep workspace files', async () => {
      const workspace = await getWorkspaceApi(request, name)
      await fs.mkdir(workspace.path, { recursive: true })
      const notePath = path.join(workspace.path, 'MEMORY.md')
      await fs.writeFile(notePath, 'memory survives reset-session\n', 'utf8')
      await restartAgentApi(request, name, 'restart')
      await restartAgentApi(request, name, 'reset_session')
      const content = await fs.readFile(notePath, 'utf8')
      expect(content).toContain('memory survives')
    })

    await test.step('Steps 8–12: Delete with keep-workspace preserves deleted history styling', async () => {
      await clickSidebarChannel(page, 'all')
      await sendAsUser(request, username, '#all', `@${name} reply once before delete`)
      await page.reload({ waitUntil: 'networkidle' })
      await deleteAgentApi(request, name, 'preserve_workspace')
      const oldHistory = await historyForUser(request, username, '#all', 50)
      expect(oldHistory.some((entry) => entry.senderName === name && entry.senderDeleted)).toBe(true)
      await createAgentApi(request, { name, runtime: 'claude', model: 'sonnet' })
      const postRecreate = await historyForUser(request, username, '#all', 50)
      expect(postRecreate.some((entry) => entry.senderName === name && entry.senderDeleted)).toBe(true)
    })
  })
})
