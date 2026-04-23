import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  reloadApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'
import { createAgentApi } from './helpers/api'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-005 Task Proposal Accept Flow
 *
 * Smoke for the full proposal -> accept -> sub-channel round-trip:
 *   - an agent proposes a task via the internal bridge endpoint
 *   - the parent channel renders an interactive proposal card (pending)
 *   - clicking `create` flips the card to `accepted` and surfaces a
 *     deep-link to the new task sub-channel
 *   - clicking the deep-link opens the sub-channel and the kickoff
 *     system message is visible
 *
 * Proposal creation is driven by a direct POST to the same
 * `/internal/agent/:agent/channels/:channel/task-proposals` endpoint the
 * shared MCP bridge calls — this is the same internal-route pattern the
 * existing `sendAsUser` helper uses. Heavy verification lives in the Rust
 * store + HTTP handler e2e tests and the frontend unit tests; this spec
 * is intentionally tight and only covers "renders + clicks through".
 */
test.describe('TSK-005', () => {
  test('Task Proposal Accept Flow @case TSK-005', async ({ page, request }) => {
    const ts = Date.now()
    const slug = `qa-proposals-${ts}`
    const agentName = `proposer-${ts}`
    const title = `investigate login 500 ${ts}`

    // Response interceptor — fails the test on any 4xx/5xx to the proposal
    // HTTP surface during the flow (the catalog-level contract).
    const failed: string[] = []
    page.on('response', (res) => {
      const url = res.url()
      if (url.includes('/task-proposals') && res.status() >= 400) {
        failed.push(`${res.status()} ${url}`)
      }
    })

    await gotoApp(page)

    await test.step('Step 1: Create a channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-005')
      await clickSidebarChannel(page, slug)
    })

    await test.step('Step 2: Create an agent record', async () => {
      // The agent only needs to exist as a row — it never actually runs in
      // this smoke. Model/runtime are arbitrary; the proposal endpoint does
      // not spawn the agent.
      await createAgentApi(request, {
        name: agentName,
        runtime: 'codex',
        model: 'gpt-5.4',
        description: 'TSK-005 proposer',
      })
    })

    await test.step('Step 3: Agent proposes a task via the internal endpoint', async () => {
      // Same internal-route pattern the shared MCP bridge uses (see
      // `src/bridge/backend.rs` -> `POST /internal/agent/:agent/channels/:channel/task-proposals`).
      // The Playwright `request` fixture already shares the per-worker
      // baseURL, so no auth or localhost binding gymnastics are required.
      const res = await request.post(
        `/internal/agent/${encodeURIComponent(agentName)}/channels/${encodeURIComponent(slug)}/task-proposals`,
        { data: { title } }
      )
      expect(res.ok(), await res.text()).toBeTruthy()
      const body = (await res.json()) as { id: string; status: string }
      expect(body.status).toBe('pending')
    })

    await test.step('Step 4: Reload — proposal card renders pending with create button', async () => {
      // Reload so the SSE/ws stream is guaranteed to have flushed the
      // system message carrying the proposal snapshot.
      await reloadApp(page)
      await clickSidebarChannel(page, slug)
    })

    const card = page.locator('[data-testid^="task-proposal-"]').first()
    await expect(card).toBeVisible()
    await expect(card).toHaveAttribute('data-status', 'pending')
    await expect(card).toContainText(title)
    await expect(
      card.locator('[data-testid="task-proposal-accept-btn"]')
    ).toBeVisible()

    await test.step('Step 5: Click create — card flips to accepted with task deep-link', async () => {
      await card.locator('[data-testid="task-proposal-accept-btn"]').click()
      // The accept mutation posts a second `kind = "task_proposal"` snapshot
      // with `status = "accepted"` into the parent channel. The reducer
      // folds both into the same card, so `data-status` transitions in
      // place — no new card appears.
      await expect(card).toHaveAttribute('data-status', 'accepted')
      await expect(card).toContainText(/task #\d+ opened in/)
      await expect(card).toContainText(`${slug}__task-`)
    })

    await test.step('Step 6: Click the deep-link — sub-channel loads with kickoff message', async () => {
      await card.locator('.task-proposal-link').click()
      // Acceptance posts a kickoff system message in the new sub-channel
      // ("Task #N opened: <title>. <agent>, you proposed ..."). Assert the
      // sub-channel view renders and the kickoff text appears.
      await expect(page.getByText(/Task #\d+ opened:/).first()).toBeVisible()
      await expect(page.getByText(title).first()).toBeVisible()
    })

    expect(failed, 'no 4xx/5xx responses on /task-proposals during the flow').toEqual([])
  })
})
