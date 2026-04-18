/**
 * Per-worker isolated server fixture.
 *
 * By default each Playwright worker starts its own `chorus serve` process on a
 * dedicated port (3200 + workerIndex) with a temporary data directory, giving
 * every worker full isolation with no shared SQLite state.
 *
 * Override with `CHORUS_BASE_URL` to point all workers at an existing server
 * (serial, shared-state mode — useful for debugging against a known dataset):
 *
 *   CHORUS_BASE_URL=http://localhost:3101 npx playwright test
 */
import { test as base } from '@playwright/test'
import { spawn, type ChildProcess } from 'child_process'
import { mkdtempSync, rmSync } from 'fs'
import { tmpdir } from 'os'
import path from 'path'

const REPO_ROOT = path.resolve(__dirname, '../../../../')
const BINARY = path.join(REPO_ROOT, 'target', 'debug', 'chorus')
const BASE_PORT = 3200

async function pollServer(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${url}/api/whoami`)
      if (res.ok) return
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 200))
  }
  throw new Error(`Server at ${url} did not become ready within ${timeoutMs}ms`)
}

// Worker-scoped fixtures (one per worker process, shared across all tests in
// that worker).
type WorkerFixtures = { workerServerUrl: string }

export const test = base.extend<Record<string, never>, WorkerFixtures>({
  workerServerUrl: [
    async ({}, use, workerInfo) => {
      const externalBaseURL = process.env.CHORUS_BASE_URL
      if (externalBaseURL) {
        // External server: no isolation, serial-safe debug path
        await use(externalBaseURL)
        return
      }

      const port = BASE_PORT + workerInfo.workerIndex
      const dataDir = mkdtempSync(path.join(tmpdir(), `chorus-qa-w${workerInfo.workerIndex}-`))

      const proc: ChildProcess = spawn(
        BINARY,
        ['serve', '--port', String(port), '--data-dir', dataDir],
        { cwd: REPO_ROOT, stdio: 'pipe' }
      )

      const serverUrl = `http://localhost:${port}`
      try {
        await pollServer(serverUrl, 30_000)
        await use(serverUrl)
      } finally {
        proc.kill('SIGTERM')
        // Give the process a moment to flush before we remove the data dir
        await new Promise((r) => setTimeout(r, 300))
        rmSync(dataDir, { recursive: true, force: true })
      }
    },
    { scope: 'worker' },
  ],

  // Override the built-in `page` fixture so each page opens in a context
  // whose baseURL points at this worker's server.
  page: async ({ browser, workerServerUrl }, use) => {
    const context = await browser.newContext({ baseURL: workerServerUrl })
    const page = await context.newPage()
    await use(page)
    await context.close()
  },

  // Override the built-in `request` fixture so API helpers hit the same
  // per-worker server.
  request: async ({ playwright, workerServerUrl }, use) => {
    const context = await playwright.request.newContext({ baseURL: workerServerUrl })
    await use(context)
    await context.dispose()
  },
})

export { expect } from '@playwright/test'
