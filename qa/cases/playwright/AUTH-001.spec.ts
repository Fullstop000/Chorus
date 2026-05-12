import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/auth.md` — AUTH-001 First-Load Session Bootstrap.
 *
 * Verifies a fresh browser with no `chorus_sid` cookie:
 * 1. Receives 401 on the initial `/api/whoami`.
 * 2. Transparently `POST`s `/api/auth/local-session` to mint a cookie.
 * 3. Retries `/api/whoami` and succeeds.
 * 4. Reuses the cookie on a page reload — exactly one mint per browser session.
 *
 * The cookie must be `HttpOnly` + `SameSite=Strict` + `Path=/`, matching
 * `src/server/auth/local_session.rs`. We verify `HttpOnly` by reading the
 * server's `Set-Cookie` header (browser hides HttpOnly cookies from JS).
 */
test.describe('AUTH-001', () => {
  test('First-Load Session Bootstrap @case AUTH-001', async ({ browser, workerServerUrl }) => {
    // Fresh context = empty cookie jar. We deliberately don't reuse the
    // override `page` fixture so we can assert on the request log from
    // the very first navigation.
    const context = await browser.newContext({ baseURL: workerServerUrl })
    try {
      const page = await context.newPage()

      const localSessionRequests: number[] = [] // captured response statuses
      const whoamiRequests: number[] = []
      let observedSetCookie: string | null = null
      page.on('response', async (res) => {
        const url = res.url()
        if (url.endsWith('/api/auth/local-session')) {
          localSessionRequests.push(res.status())
          if (res.status() === 200) {
            // `Set-Cookie` is filtered out of `response.headers()` per
            // Playwright's normal header view; use the raw header API.
            const all = await res.headersArray()
            const cookie = all.find((h) => h.name.toLowerCase() === 'set-cookie')?.value
            if (cookie) observedSetCookie = cookie
          }
        }
        if (url.endsWith('/api/whoami')) {
          whoamiRequests.push(res.status())
        }
      })

      await test.step('Step 1: Open the app root URL', async () => {
        await gotoApp(page)
      })

      await test.step('Step 2: /api/auth/local-session mints a cookie', async () => {
        await expect
          .poll(() => localSessionRequests, { timeout: 10_000 })
          .toContain(200)
        expect(observedSetCookie, 'expected Set-Cookie from local-session').not.toBeNull()
        const cookie = observedSetCookie!
        expect(cookie, `set-cookie shape: ${cookie}`).toMatch(/^chorus_sid=ses_/)
        expect(cookie, `set-cookie must be HttpOnly: ${cookie}`).toMatch(/HttpOnly/)
        expect(cookie, `set-cookie must be SameSite=Strict: ${cookie}`).toMatch(/SameSite=Strict/)
        expect(cookie, `set-cookie must be Path=/: ${cookie}`).toMatch(/Path=\//)
      })

      await test.step('Step 3: /api/whoami succeeds after bootstrap', async () => {
        await expect
          .poll(() => whoamiRequests, { timeout: 10_000 })
          .toContain(200)
      })

      await test.step('Step 4: Sidebar footer renders the local user', async () => {
        await expect(page.locator('.sidebar-footer')).toBeVisible()
        await expect(page.locator('.you-badge')).toBeVisible()
      })

      const mintsBeforeReload = localSessionRequests.length

      await test.step('Step 5: Reload — cookie reused, no second mint', async () => {
        await page.reload()
        await expect(page.locator('.sidebar-footer')).toBeVisible()
        // The reload's /api/whoami should succeed with the existing cookie.
        // No second POST /api/auth/local-session is expected — but we
        // give the page a beat to settle before asserting.
        await page.waitForTimeout(500)
        expect(
          localSessionRequests.length,
          `expected exactly ${mintsBeforeReload} local-session mints, observed: ${localSessionRequests.length}`,
        ).toBe(mintsBeforeReload)
      })
    } finally {
      await context.close()
    }
  })
})
