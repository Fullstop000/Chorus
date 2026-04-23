import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-004 Channel Task-Event Feed
 *
 * Verifies the channel chat tab shows task activity as a living card that
 * mutates through the lifecycle and collapses on done:
 *   - creating a task surfaces a card in the parent channel chat
 *   - claim / advance / done each update the SAME card in place
 *   - terminal done collapses to the compact done-pill
 */
test.describe('TSK-004', () => {
  test('Channel Task-Event Feed @case TSK-004', async ({ page }) => {
    const slug = `qa-task-feed-${Date.now()}`
    const title = `TSK-004 ${Date.now()}`

    const failed: string[] = []
    page.on('response', (res) => {
      const url = res.url()
      if (
        (url.includes('/tasks') || url.includes('/channels')) &&
        res.status() >= 400
      ) {
        failed.push(`${res.status()} ${url}`)
      }
    })

    await gotoApp(page)

    await test.step('Step 1: Create a channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-004')
      await clickSidebarChannel(page, slug)
    })

    await test.step('Step 2: Create a task via the kanban tab', async () => {
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await page.locator('.new-task-input').fill(title)
      await page.locator('.new-task-submit').click()
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible()
    })

    await test.step('Step 3: Parent channel chat shows a task card in todo', async () => {
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      const thread = page.locator('[data-testid^="task-thread-"]').first()
      await expect(thread).toBeVisible()
      await expect(thread).toHaveAttribute('data-state', 'todo')
      await expect(thread).toContainText(title)
    })

    const thread = page.locator('[data-testid^="task-thread-"]').first()

    await test.step('Step 4: Start the task (todo → in_progress) — card mutates in place', async () => {
      await thread.click()
      await page.getByRole('button', { name: /start/i }).click()
      await page.getByRole('button', { name: /back to channel/i }).click()
      await expect(thread).toHaveAttribute('data-state', 'in_progress')
    })

    await test.step('Step 5: Submit for review — status pill updates', async () => {
      await thread.click()
      await page.getByRole('button', { name: /submit for review/i }).click()
      await page.getByRole('button', { name: /back to channel/i }).click()
      await expect(thread).toHaveAttribute('data-state', 'in_review')
    })

    await test.step('Step 6: Mark done — card collapses to done pill', async () => {
      await thread.click()
      await page.getByRole('button', { name: /mark done/i }).click()
      await page.getByRole('button', { name: /back to channel/i }).click()
      await expect(thread).toHaveAttribute('data-state', 'done')
      await expect(thread.locator('.task-event-done-row')).toBeVisible()
      await expect(thread.locator('.task-event-done-row')).toContainText(title)
    })

    expect(failed, 'no 4xx/5xx responses during the lifecycle').toEqual([])
  })
})
