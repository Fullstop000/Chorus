import { test, expect } from './helpers/fixtures'
import { createUserChannelViaUi, clickSidebarChannel, sendChatMessage , gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-002 Create Message-As-Task From Composer
 */
test.describe('TSK-002', () => {
  test('Create Message-As-Task From Composer @case TSK-002', async ({ page }) => {
    const slug = `qa-task-msg-${Date.now()}`
    const title = `Task from composer ${Date.now()}`
    await gotoApp(page)

    await test.step('Precondition: open disposable channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-002')
      await clickSidebarChannel(page, slug)
    })

    await test.step('Steps 1–3: Send message with Also create as a task enabled', async () => {
      await page.locator('.task-checkbox-label input').check()
      await sendChatMessage(page, title)
      await expect(page.locator('.message-item').filter({ hasText: title }).first()).toBeVisible()
    })

    await test.step('Step 4: Matching task exists in Tasks tab', async () => {
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await expect(
        page.locator('.task-column[data-status="todo"] .task-card-title').filter({ hasText: title })
      ).toBeVisible()
    })
  })
})
