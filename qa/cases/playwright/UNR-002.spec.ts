import { test, expect } from './helpers/fixtures'
import type { APIRequestContext, Locator } from '@playwright/test'
import {
  createAgentApi,
  getWhoami,
  inviteChannelMemberApi,
  listAgents,
  listChannelsApi,
} from './helpers/api'
import {
  clickSidebarChannel,
  createUserChannelViaUi,
  gotoApp,
  sendChatMessage,
} from './helpers/ui'

async function postMessage(
  request: APIRequestContext,
  actor: string,
  target: string,
  content: string,
  options?: { suppressAgentDelivery?: boolean },
): Promise<{ messageId: string; seq: number }> {
  const response = await request.post(`/internal/agent/${encodeURIComponent(actor)}/send`, {
    data: {
      target,
      content,
      suppressAgentDelivery: options?.suppressAgentDelivery ?? false,
    },
  })
  expect(response.ok(), await response.text()).toBeTruthy()
  return response.json()
}

async function readBadge(locator: Locator): Promise<number> {
  if (await locator.count() === 0) return 0
  const text = (await locator.textContent())?.trim() ?? '0'
  return Number(text)
}

/**
 * UNR-002 / UNR-003 / UNR-004 / UNR-005 — Unread Tracking Refactor E2E
 *
 * Test matrix:
 *
 *   UNR-002   [pre-refactor]  PASSES NOW — proves per-message inbox fetches exist today.
 *   UNR-002b  [post-refactor] FAILS NOW  — asserts NO inbox fetches; passes after refactor removes them.
 *   UNR-003   [post-refactor] FAILS NOW  — badge = unreadMessageIds.size; clears on scroll-to-bottom.
 *   UNR-004   [post-refactor] FAILS NOW  — read-cursor fires but no inbox refetch on clear.
 *   UNR-005   [post-refactor] FAILS NOW  — markUnreadAsSeen reduces count as messages scroll into view.
 */
test.describe('UNR-002/003', () => {
  test('[pre-refactor] per-message inbox-notification fetches fire when messages arrive @case UNR-002', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `unr002-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `unr002-${Date.now()}`

    await gotoApp(page)
    await createUserChannelViaUi(page, channelName, 'pre-refactor coverage')

    const created = (await listChannelsApi(request, { member: username, includeDm: true }))
      .find((ch) => ch.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    const inboxNotifUrls: Array<string> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (req.method() === 'GET' && url.pathname.includes('inbox-notification')) {
        inboxNotifUrls.push(url.pathname)
      }
    })

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    for (let i = 0; i < 10; i++) {
      await sendChatMessage(page, `seed-${Date.now()}-${i}`)
    }
    await expect(page.locator('.message-item').last()).toBeVisible({ timeout: 15_000 })

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const n = 6
    for (let i = 0; i < n; i++) {
      await postMessage(request, agentName, `#${channelName}`, `u-${Date.now()}-${i}`, {
        suppressAgentDelivery: true,
      })
    }

    const row = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(row.locator('.sidebar-unread-badge')).toHaveText(String(n), { timeout: 30_000 })

    expect(inboxNotifUrls.length).toBeGreaterThan(0)
  })

  test('[post-refactor] zero inbox-notification fetches when messages arrive @case UNR-002b', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `unr002b-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `unr002b-${Date.now()}`

    await gotoApp(page)
    await createUserChannelViaUi(page, channelName, 'no-inbox-notif')

    const created = (await listChannelsApi(request, { member: username, includeDm: true }))
      .find((ch) => ch.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    const inboxNotifUrls: Array<string> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (req.method() === 'GET' && url.pathname.includes('inbox-notification')) {
        inboxNotifUrls.push(url.pathname)
      }
    })

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    for (let i = 0; i < 10; i++) {
      await sendChatMessage(page, `s-${Date.now()}-${i}`)
    }
    await expect(page.locator('.message-item').last()).toBeVisible({ timeout: 15_000 })

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    for (let i = 0; i < 6; i++) {
      await postMessage(request, agentName, `#${channelName}`, `u-${Date.now()}-${i}`, {
        suppressAgentDelivery: true,
      })
    }

    const row = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(row.locator('.sidebar-unread-badge')).toHaveText('6', { timeout: 30_000 })
    expect(inboxNotifUrls.length).toBe(0)
  })

  test('[post-refactor] badge count equals unread message count; clears on scroll-to-bottom @case UNR-003', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `unr003-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `unr003-${Date.now()}`

    await gotoApp(page)
    await createUserChannelViaUi(page, channelName, 'badge-equals-count')

    const created = (await listChannelsApi(request, { member: username, includeDm: true }))
      .find((ch) => ch.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    for (let i = 0; i < 8; i++) {
      await sendChatMessage(page, `s-${Date.now()}-${i}`)
    }

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const n = 12
    for (let i = 0; i < n; i++) {
      await postMessage(request, agentName, `#${channelName}`, `u-${Date.now()}-${i}`, {
        suppressAgentDelivery: true,
      })
    }

    const row = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(row.locator('.sidebar-unread-badge')).toHaveText(String(n), { timeout: 30_000 })

    const before = await readBadge(row.locator('.sidebar-unread-badge'))
    expect(before).toBe(n)

    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await page.locator('.message-list').evaluate((el) => {
      ;(el as HTMLElement).scrollTop = el.scrollHeight
    })
    await expect(row.locator('.sidebar-unread-badge')).toHaveCount(0)
  })

  test('[post-refactor] read-cursor dispatched on clear but inbox NOT re-fetched @case UNR-004', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `unr004-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `unr004-${Date.now()}`

    await gotoApp(page)
    await createUserChannelViaUi(page, channelName, 'no-inbox-refetch')

    const created = (await listChannelsApi(request, { member: username, includeDm: true }))
      .find((ch) => ch.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    const readCursorPosts: unknown[] = []
    const inboxGetUrls: Array<string> = []
    page.on('request', (req) => {
      const url = new URL(req.url())
      if (
        req.method() === 'POST' &&
        url.pathname === `/api/conversations/${created!.id}/read-cursor`
      ) {
        readCursorPosts.push(req.postDataJSON())
      }
      if (
        req.method() === 'GET' &&
        (url.pathname.includes('/inbox') || url.pathname.includes('inbox-notification'))
      ) {
        inboxGetUrls.push(url.pathname)
      }
    })

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    for (let i = 0; i < 6; i++) {
      await sendChatMessage(page, `s-${Date.now()}-${i}`)
    }
    await expect(page.locator('.message-item').last()).toBeVisible({ timeout: 15_000 })

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    for (let i = 0; i < 5; i++) {
      await postMessage(request, agentName, `#${channelName}`, `u-${Date.now()}-${i}`, {
        suppressAgentDelivery: true,
      })
    }

    const row = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(row.locator('.sidebar-unread-badge')).toHaveText('5', { timeout: 30_000 })

    const getsBeforeClear = inboxGetUrls.length

    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)
    await page.locator('.message-list').evaluate((el) => {
      ;(el as HTMLElement).scrollTop = el.scrollHeight
    })

    await expect(readCursorPosts.length).toBeGreaterThanOrEqual(1)
    expect(inboxGetUrls.length).toBe(getsBeforeClear)
  })

  test('[post-refactor] badge decreases as individual messages scroll into view @case UNR-005', async ({
    page,
    request,
  }) => {
    const { username } = await getWhoami(request)
    let agentName = (await listAgents(request))[0]?.name
    if (!agentName) {
      agentName = `unr005-bot-${Date.now()}`
      await createAgentApi(request, {
        name: agentName,
        runtime: 'claude',
        model: 'sonnet',
      })
    }
    const channelName = `unr005-${Date.now()}`

    await gotoApp(page)
    await createUserChannelViaUi(page, channelName, 'per-msg-seen')

    const created = (await listChannelsApi(request, { member: username, includeDm: true }))
      .find((ch) => ch.name === channelName)
    expect(created?.id).toBeTruthy()
    await inviteChannelMemberApi(request, created!.id!, agentName)

    await clickSidebarChannel(page, channelName)
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    for (let i = 0; i < 6; i++) {
      await sendChatMessage(page, `s-${Date.now()}-${i}`)
    }

    await page.getByRole('button', { name: 'Tasks', exact: true }).click()

    const total = 18
    for (let i = 0; i < total; i++) {
      await postMessage(request, agentName, `#${channelName}`, `u${Date.now()}-${i}`, {
        suppressAgentDelivery: true,
      })
    }

    const row = page.locator('.sidebar-channel-row').filter({ hasText: channelName }).first()
    await expect(row.locator('.sidebar-unread-badge')).toHaveText(String(total), { timeout: 30_000 })

    await page.getByRole('button', { name: 'Chat', exact: true }).click()
    await expect(page.locator('.chat-header-name')).toContainText(`#${channelName}`)

    const full = await readBadge(row.locator('.sidebar-unread-badge'))
    expect(full).toBe(total)

    const msgList = page.locator('.message-list')
    const itemH = await msgList.locator('.message-item').first().evaluate(
      (el) => (el as HTMLElement).offsetHeight,
    )
    await msgList.evaluate((el, h) => {
      ;(el as HTMLElement).scrollTop = h * 4
    }, itemH)

    await page.waitForTimeout(5000)

    const partial = await readBadge(row.locator('.sidebar-unread-badge'))
    expect(partial).toBeGreaterThan(0)
    expect(partial).toBeLessThan(total)

    await msgList.evaluate((el) => {
      ;(el as HTMLElement).scrollTop = el.scrollHeight
    })
    await expect(row.locator('.sidebar-unread-badge')).toHaveCount(0)
  })
})
