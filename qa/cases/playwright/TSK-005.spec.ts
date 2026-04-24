import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  reloadApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'
import {
  createAgentApi,
  getWhoami,
  historyForUser,
  pollUntil,
  sendAsUser,
} from './helpers/api'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-005 Task Proposal Accept Flow
 *
 * Smoke for the full proposal -> accept -> sub-channel round-trip:
 *   - a human seeds the originating ask in the parent channel
 *   - an agent proposes a task against that message via the internal
 *     bridge endpoint (passing `sourceMessageId` — v2 required field)
 *   - the parent channel renders an interactive proposal card (pending)
 *   - clicking `create` flips the card to `accepted` and surfaces a
 *     deep-link to the new task sub-channel
 *   - clicking the deep-link opens the sub-channel and the kickoff
 *     system message carries the v2 context snapshot (title + author
 *     attribution + verbatim blockquote) in that order
 *
 * Proposal creation is driven by a direct POST to the same
 * `/internal/agent/:agent/channels/:channel/task-proposals` endpoint the
 * shared MCP bridge calls — this is the same internal-route pattern the
 * existing `sendAsUser` helper uses. Heavy verification lives in the Rust
 * store + HTTP handler e2e tests and the frontend unit tests; this spec
 * is intentionally tight and only covers "renders + clicks through" plus
 * the v2 kickoff ordering contract.
 */
test.describe('TSK-005', () => {
  test('Task Proposal Accept Flow @case TSK-005', async ({ page, request }) => {
    const ts = Date.now()
    const slug = `qa-proposals-${ts}`
    // The server may auto-suffix the agent name on create (collision or
    // sanitization). Capture the actual stored name from the response
    // rather than trusting the requested value — the membership invite
    // and propose path both use the server-canonical name.
    let agentName = `proposer-${ts}`
    const title = `investigate login 500 ${ts}`
    const sourceContent = 'the login form breaks on Safari mobile'

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
      const created = await createAgentApi(request, {
        name: agentName,
        runtime: 'codex',
        model: 'gpt-5.4',
        description: 'TSK-005 proposer',
      })
      agentName = created.name
    })

    await test.step('Step 2b: Join the agent to the channel', async () => {
      // v2 enforces channel membership on `internal_agent_propose` — an
      // agent that isn't a member of the target channel is rejected with
      // 403 MESSAGE_NOT_A_MEMBER, matching the send-path precondition.
      // This smoke seeds the membership row directly against the public
      // members endpoint rather than driving the UI, because the catalog-
      // level assertion is the proposal round-trip, not the invite flow.
      // The invite endpoint accepts `memberType: "agent"`; the sibling
      // `inviteChannelMemberApi` helper only passes `memberName` (human-
      // only), so we post here directly.
      const channelsList = await request.get('/api/channels')
      expect(channelsList.ok(), await channelsList.text()).toBeTruthy()
      const channels = (await channelsList.json()) as Array<{
        id: string
        name: string
      }>
      const chRow = channels.find((c) => c.name === slug)
      expect(chRow, `channel ${slug} not found`).toBeTruthy()
      const joinRes = await request.post(
        `/api/channels/${encodeURIComponent(chRow!.id)}/members`,
        { data: { memberName: agentName, memberType: 'agent' } }
      )
      expect(joinRes.ok(), await joinRes.text()).toBeTruthy()
    })

    // v2 requires `sourceMessageId`: the proposal snapshots the originating
    // human message so the per-task session has it as immutable kickoff
    // context. Seed the source message as the logged-in human, then fish
    // its id out of channel history.
    const { username } = await getWhoami(request)
    let sourceMessageId = ''

    await test.step('Step 3: Human seeds the source message in the parent channel', async () => {
      await sendAsUser(request, username, `#${slug}`, sourceContent)
      sourceMessageId = await pollUntil(async () => {
        const msgs = await historyForUser(request, username, `#${slug}`, 40)
        const match = msgs.find(
          (m) => m.senderName === username && m.content === sourceContent
        )
        return match?.id
      }, 15_000)
      expect(sourceMessageId).toBeTruthy()
    })

    await test.step('Step 4: Agent proposes a task against that message', async () => {
      // Same internal-route pattern the shared MCP bridge uses (see
      // `src/bridge/backend.rs` -> `POST /internal/agent/:agent/channels/:channel/task-proposals`).
      // The Playwright `request` fixture already shares the per-worker
      // baseURL, so no auth or localhost binding gymnastics are required.
      const res = await request.post(
        `/internal/agent/${encodeURIComponent(agentName)}/channels/${encodeURIComponent(slug)}/task-proposals`,
        { data: { title, sourceMessageId } }
      )
      expect(res.ok(), await res.text()).toBeTruthy()
      const body = (await res.json()) as { id: string; status: string }
      expect(body.status).toBe('pending')
    })

    await test.step('Step 5: Reload — proposal card renders pending with create button', async () => {
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

    await test.step('Step 6: Click create — card flips to accepted with task deep-link', async () => {
      await card.locator('[data-testid="task-proposal-accept-btn"]').click()
      // The accept mutation posts a second `kind = "task_proposal"` snapshot
      // with `status = "accepted"` into the parent channel. The reducer
      // folds both into the same card, so `data-status` transitions in
      // place — no new card appears.
      await expect(card).toHaveAttribute('data-status', 'accepted')
      await expect(card).toContainText(/task #\d+ opened in/)
      await expect(card).toContainText(`${slug}__task-`)
    })

    await test.step('Step 7: Deep-link opens sub-channel; kickoff carries ordered snapshot sections', async () => {
      await card.locator('.task-proposal-link').click()

      // The v2 kickoff is a single system message composed of three ordered
      // sections: title line, author+parent attribution, blockquoted
      // verbatim source content. All three must appear, and in that order.
      // Locate by the first-section marker, then extract the full innerText
      // once and assert substring order — multi-line system-message content
      // collapses whitespace in `toContainText`, which would hide ordering
      // bugs.
      const kickoff = page.locator('.system-message-divider__label').filter({
        hasText: `Task opened: ${title}`,
      }).first()
      await expect(kickoff).toBeVisible()

      const text = (await kickoff.innerText()).replace(/\s+/g, ' ')
      const expectedTitle = `Task opened: ${title}`
      const expectedAttribution = `From @${username}'s message in #${slug}:`
      const expectedQuote = `> ${sourceContent}`

      const titleIdx = text.indexOf(expectedTitle)
      const attrIdx = text.indexOf(expectedAttribution)
      const quoteIdx = text.indexOf(expectedQuote)

      expect(titleIdx, `missing "${expectedTitle}" in kickoff`).toBeGreaterThanOrEqual(0)
      expect(attrIdx, `missing "${expectedAttribution}" in kickoff`).toBeGreaterThan(titleIdx)
      expect(quoteIdx, `missing blockquoted source in kickoff`).toBeGreaterThan(attrIdx)
    })

    expect(failed, 'no 4xx/5xx responses on /task-proposals during the flow').toEqual([])
  })
})
