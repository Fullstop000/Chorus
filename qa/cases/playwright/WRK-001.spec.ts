import { test, expect } from './helpers/fixtures'
import fs from 'node:fs/promises'
import path from 'node:path'
import {
  ensureMixedRuntimeTrio,
  getWorkspaceApi,
  getWorkspaceFileApi,
  type TrioNames,
} from './helpers/api'
import { openAgentTab , gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — WRK-001 Workspace Tab Path And File Visibility
 */
test.describe('WRK-001', () => {
  let trio: TrioNames

  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Workspace Tab Path And File Visibility @case WRK-001', async ({ page, request }) => {
    const workspace = await getWorkspaceApi(request, trio.botA)
    const notesDir = path.join(workspace.path, 'notes')
    const noteRel = 'notes/work-log.md'
    const noteAbs = path.join(workspace.path, noteRel)
    await fs.mkdir(notesDir, { recursive: true })
    await fs.writeFile(noteAbs, '# Work Log\n\n- qa workspace note\n', 'utf8')
    const preview = await getWorkspaceFileApi(request, trio.botA, noteRel)

    await gotoApp(page)
    await openAgentTab(page, trio.displayA, 'Workspace')

    await test.step('Steps 1–6: Path, tree, and file metadata are visible', async () => {
      await expect(page.locator('.workspace-location')).toContainText(workspace.path)
      await expect(page.locator('.workspace-row-label').filter({ hasText: 'notes' })).toBeVisible()
      await page.locator('.workspace-row').filter({ hasText: 'notes' }).first().click()
      await page.locator('.workspace-row').filter({ hasText: 'work-log.md' }).first().click()
      await expect(page.locator('.workspace-preview-title')).toContainText(noteRel)
      await expect(page.locator('.workspace-preview-detail').first()).toContainText('B')
    })

    await test.step('Steps 7–10: Raw and Preview modes both work and refresh keeps view usable', async () => {
      await page.getByRole('button', { name: 'Raw' }).click()
      await expect(page.locator('.workspace-preview-content')).toContainText('# Work Log')
      await page.getByRole('button', { name: 'Preview' }).click()
      await expect(page.locator('.workspace-markdown')).toContainText('Work Log')
      await page.getByRole('button', { name: 'Refresh workspace' }).click()
      await expect(page.locator('.workspace-preview-title')).toContainText(noteRel)
    })

    expect(preview.content).toContain('qa workspace note')
  })
})
