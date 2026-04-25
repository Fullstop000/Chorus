import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
  reloadApp,
} from './helpers/ui'
import {
  ensureMixedRuntimeTrio,
  getWhoami,
  proposeTaskAsAgent,
  sendAsUserGetId,
} from './helpers/api'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-005 Agent Proposal And Snapshot Kickoff
 *
 * Validates the proposal half of the unified lifecycle:
 *   - the agent-scoped `POST /internal/agent/{agent}/tasks/propose` endpoint
 *     snapshots the source message into the proposal
 *   - the parent-channel TaskCard renders in `proposed` with the snapshot
 *     blockquote (sender + content)
 *   - clicking `[create]` flips the card to `todo` and mints a sub-channel
 *   - the `task-card-link` deep-link opens the sub-channel
 *   - the kickoff system message contains the three snapshot sections in
 *     order: "Task opened: {title}" → "From @{sender}'s message in #{parent}:"
 *     → "> {content}"
 */
test.describe('TSK-005', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Agent Proposal And Snapshot Kickoff @case TSK-005', async ({ page, request }) => {
    const trio = await ensureMixedRuntimeTrio(request)
    const slug = `qa-task-prop-${Date.now()}`
    const title = `TSK-005 ${Date.now()}`
    const sourceContent = `seed message ${Date.now()}`

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

    await test.step('Step 1: Create a parent channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-005')
      await clickSidebarChannel(page, slug)
    })

    // Seed the source chat message via the API so we can capture its id and
    // hand it to the propose endpoint. The human is the source sender; the
    // agent (bot-a) is the proposer.
    const { username } = await getWhoami(request)
    const sourceMessageId = await sendAsUserGetId(
      request,
      username,
      `#${slug}`,
      sourceContent,
    )

    await test.step('Step 2: Agent proposes a task tied to the source message', async () => {
      await proposeTaskAsAgent(request, trio.botA, {
        channel: `#${slug}`,
        title,
        sourceMessageId,
      })
    })

    // Reload so the freshly inserted parent-channel `task_card` system message
    // and the proposed task row are both pulled into the UI from history.
    await reloadApp(page)
    await clickSidebarChannel(page, slug)

    // Visiting the Tasks tab populates the global tasksStore via useTasks
    // polling, so the parent-channel TaskCard host can resolve `useTask(id)`
    // to the proposed row. Without this hop the card renders as null.
    await page.getByRole('button', { name: 'Tasks', exact: true }).click()
    await page.getByRole('button', { name: 'Chat', exact: true }).click()

    const card = page
      .locator('[data-testid^="task-card-"]')
      .filter({ hasText: title })
      .first()

    await test.step('Step 3: Parent channel TaskCard renders in proposed with snapshot', async () => {
      await expect(card).toBeVisible({ timeout: 15_000 })
      await expect(card).toHaveAttribute('data-status', 'proposed')
      await expect(card).toContainText(title)
      // Snapshot blockquote: sender label + verbatim content.
      await expect(card.locator('blockquote')).toBeVisible()
      await expect(card).toContainText(username)
      await expect(card).toContainText(sourceContent)
    })

    await test.step('Step 4: Click [create]; card flips to todo with claim CTA', async () => {
      await card.locator('[data-testid="task-card-accept-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'todo', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-claim-btn"]')).toBeVisible()
    })

    await test.step('Step 5: Advance to in_progress to surface the sub-channel deep-link', async () => {
      // The TaskCard `[task-card-link]` deep-link only renders on
      // in_progress / in_review / done — `todo` has no link slot. Advancing
      // (claim → start) gets us there without leaving the chat surface.
      await card.locator('[data-testid="task-card-claim-btn"]').click()
      await expect(card).toHaveAttribute('data-claimed', 'true', { timeout: 15_000 })
      await card.locator('[data-testid="task-card-start-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_progress', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-link"]')).toBeVisible()
    })

    await test.step('Step 6: Click deep-link; kickoff carries the three snapshot sections', async () => {
      await card.locator('[data-testid="task-card-link"]').click()
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible()

      // The kickoff system message renders as a generic `.message-item` in the
      // sub-channel feed; all three sections are concatenated into one body.
      const kickoff = page
        .locator('[data-testid="task-detail"] .message-item')
        .filter({ hasText: `Task opened: ${title}` })
        .first()
      await expect(kickoff).toBeVisible({ timeout: 15_000 })
      await expect(kickoff).toContainText(`From @${username}'s message in #${slug}:`)
      await expect(kickoff).toContainText(`> ${sourceContent}`)
    })

    expect(failed, failed.join('; ')).toEqual([])
  })
})
