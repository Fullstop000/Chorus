/**
 * Per-worker bearer-token registry.
 *
 * Lives in its own module so both `fixtures.ts` (which writes to the
 * registry at server-startup time) and `api.ts` (which reads from it
 * inside `/internal/agent/*` helpers) can import it without creating
 * a cycle through Playwright's test-runner module. A cycle here breaks
 * single-spec test discovery with a misleading "No tests found" error.
 *
 * Each Playwright worker is its own Node process, so module state is
 * automatically per-worker.
 */

const bridgeTokensByServerUrl = new Map<string, string>()

export function registerBridgeToken(serverUrl: string, token: string): void {
  bridgeTokensByServerUrl.set(serverUrl, token)
}

export function unregisterBridgeToken(serverUrl: string): void {
  bridgeTokensByServerUrl.delete(serverUrl)
}

export function getBridgeTokenForServer(serverUrl: string): string | undefined {
  return bridgeTokensByServerUrl.get(serverUrl)
}

/**
 * Each Playwright worker provisions exactly one chorus server, so the
 * map has exactly one entry once setup runs. Helpers that only see an
 * `APIRequestContext` (not the worker's server URL) call this to recover
 * the bridge token.
 */
export function getCurrentWorkerBridgeToken(): string | undefined {
  if (bridgeTokensByServerUrl.size === 1) {
    return bridgeTokensByServerUrl.values().next().value
  }
  return undefined
}
