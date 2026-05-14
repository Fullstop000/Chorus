/**
 * Per-worker isolated server fixture.
 *
 * By default each Playwright worker starts its own `chorus-server` process on
 * a dedicated port (3200 + workerIndex) with a temporary data directory,
 * giving every worker full isolation with no shared SQLite state.
 *
 * Override with `CHORUS_BASE_URL` to point all workers at an existing server
 * (serial, shared-state mode — useful for debugging against a known dataset):
 *
 *   CHORUS_BASE_URL=http://localhost:3101 npx playwright test
 */
import { test as base } from '@playwright/test'
import { spawn, spawnSync, type ChildProcess } from 'child_process'
import { createWriteStream, mkdtempSync, readFileSync, rmSync } from 'fs'
import { tmpdir } from 'os'
import path from 'path'
import { registerBridgeToken, unregisterBridgeToken } from './tokens'

const REPO_ROOT = path.resolve(__dirname, '../../../../')
const BINARY = path.join(REPO_ROOT, 'target', 'debug', 'chorus-server')
const BASE_PORT = 3200
const BASE_BRIDGE_PORT = 4400

// Per-worker bookkeeping: `workerCliToken` needs the data dir
// `workerServerUrl` provisioned. Module scope = per worker (each Playwright
// worker is its own Node process).
const workerDataDirs = new Map<number, string>()

/**
 * Run `chorus-server setup --yes` against the worker's data dir so a local
 * Account + CLI token + bridge token exist before the server starts. Without
 * this, `/api/whoami` 401s on every request (no actor) and the UI's
 * `/api/auth/local-session` bootstrap returns 409 ("no local account").
 *
 * Poll `/health` instead of `/api/whoami` so readiness doesn't depend on
 * auth state. `/health` is registered outside the `require_auth` layer.
 */
function setupDataDir(dataDir: string): void {
  const result = spawnSync(BINARY, ['setup', '--yes', '--data-dir', dataDir], {
    cwd: REPO_ROOT,
    stdio: 'pipe',
    encoding: 'utf-8',
  })
  if (result.status !== 0) {
    throw new Error(
      `chorus-server setup failed (exit ${result.status}):\nstdout: ${result.stdout}\nstderr: ${result.stderr}`,
    )
  }
}

/**
 * Read the CLI bearer token that `chorus-server setup --yes` wrote to
 * `credentials.toml`. Used to pre-authenticate the `request` fixture so
 * existing helpers that hit `/api/*` keep working without each one
 * threading auth through.
 */
function readCliToken(dataDir: string): string {
  const credsPath = path.join(dataDir, 'credentials.toml')
  const raw = readFileSync(credsPath, 'utf-8')
  // credentials.toml shape: `token = "chrs_..."` — parse cheaply.
  const match = raw.match(/^token\s*=\s*"([^"]+)"/m)
  if (!match) {
    throw new Error(`failed to extract token from ${credsPath}:\n${raw}`)
  }
  return match[1]
}

/**
 * Read the bridge bearer token from `bridge-credentials.toml`. Helpers that
 * hit `/internal/agent/<agent>/*` need this — the CLI token only authorizes
 * `/internal/agent/<user.id>/*` (CliAllowed branch).
 */
function readBridgeToken(dataDir: string): string {
  const credsPath = path.join(dataDir, 'bridge-credentials.toml')
  const raw = readFileSync(credsPath, 'utf-8')
  const match = raw.match(/^token\s*=\s*"([^"]+)"/m)
  if (!match) {
    throw new Error(`failed to extract bridge token from ${credsPath}:\n${raw}`)
  }
  return match[1]
}

// Bridge-token registry lives in `helpers/tokens.ts` so `api.ts` can
// import it without going through this fixtures module (which extends
// Playwright's test object and must not appear in a `*.spec.ts` import
// cycle — single-spec discovery breaks with a misleading "No tests found"
// error otherwise).

async function pollServer(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${url}/health`)
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
type WorkerFixtures = {
  workerServerUrl: string
  /** Bearer token for the worker's local install, written by `chorus-server setup`.
   *  Empty when CHORUS_BASE_URL is set (external server) — tests using the
   *  `request` fixture against an external server must seed their own auth. */
  workerCliToken: string
}

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
      const bridgePort = BASE_BRIDGE_PORT + workerInfo.workerIndex
      const dataDir = mkdtempSync(path.join(tmpdir(), `chorus-qa-w${workerInfo.workerIndex}-`))

      // Seed the local identity (User + Account + tokens) before serve so
      // the UI's first-load `/api/auth/local-session` mint succeeds and
      // `credentials.toml` exists for the `workerCliToken` fixture.
      setupDataDir(dataDir)

      const proc: ChildProcess = spawn(
        BINARY,
        ['--port', String(port), '--bridge-port', String(bridgePort), '--data-dir', dataDir],
        { cwd: REPO_ROOT, stdio: 'pipe' }
      )
      // Optional log capture: set CHORUS_QA_LOG_DIR=/path/to/dir to dump
      // per-worker stdout+stderr there. Cheap diagnostic for parallel
      // flakes where the failing worker's logs would otherwise be lost.
      if (process.env.CHORUS_QA_LOG_DIR) {
        const logFile = path.join(process.env.CHORUS_QA_LOG_DIR, `worker-${workerInfo.workerIndex}.log`)
        const logStream = createWriteStream(logFile, { flags: 'a' })
        proc.stdout?.pipe(logStream)
        proc.stderr?.pipe(logStream)
      }

      const serverUrl = `http://localhost:${port}`
      workerDataDirs.set(workerInfo.workerIndex, dataDir)
      // Register the bridge token by serverUrl so helpers that don't see
      // the fixture object (most of `helpers/api.ts`) can retrieve it.
      try {
        registerBridgeToken(serverUrl, readBridgeToken(dataDir))
      } catch (err) {
        // Surface the failure rather than silently leaving helpers without
        // a bridge token — every test that hits /internal/agent/<agent>/*
        // would 401 with a confusing message.
        throw new Error(`failed to read bridge token after chorus-server setup: ${err}`)
      }
      try {
        await pollServer(serverUrl, 30_000)
        await use(serverUrl)
      } finally {
        proc.kill('SIGTERM')
        // Give the process a moment to flush before we remove the data dir
        await new Promise((r) => setTimeout(r, 300))
        rmSync(dataDir, { recursive: true, force: true })
        workerDataDirs.delete(workerInfo.workerIndex)
        unregisterBridgeToken(serverUrl)
      }
    },
    { scope: 'worker' },
  ],

  workerCliToken: [
    async ({ workerServerUrl }, use, workerInfo) => {
      // workerServerUrl runs first and registers the data dir.
      void workerServerUrl
      const dataDir = workerDataDirs.get(workerInfo.workerIndex)
      const token = dataDir ? readCliToken(dataDir) : ''
      await use(token)
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
  // per-worker server with the worker's bearer token. The token comes from
  // `chorus-server setup --yes` (writes `credentials.toml`); tests using
  // `playwright.request.newContext` directly opt out of pre-auth.
  request: async ({ playwright, workerServerUrl, workerCliToken }, use) => {
    const context = await playwright.request.newContext({
      baseURL: workerServerUrl,
      extraHTTPHeaders: workerCliToken ? { Authorization: `Bearer ${workerCliToken}` } : undefined,
    })
    await use(context)
    await context.dispose()
  },
})

export { expect } from '@playwright/test'
