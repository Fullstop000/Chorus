const inflightRequests = new Map<string, Promise<unknown>>()
const recentResults = new Map<string, { value: unknown; expiresAt: number }>()

const RECENT_RESULT_TTL_MS = 250

export function loadSharedRequest<T>(key: string, loader: () => Promise<T>): Promise<T> {
  const now = Date.now()
  const recent = recentResults.get(key)
  if (recent && recent.expiresAt > now) {
    return Promise.resolve(recent.value as T)
  }

  const inflight = inflightRequests.get(key)
  if (inflight) {
    return inflight as Promise<T>
  }

  const request = loader()
    .then((value) => {
      recentResults.set(key, {
        value,
        expiresAt: Date.now() + RECENT_RESULT_TTL_MS,
      })
      return value
    })
    .finally(() => {
      if (inflightRequests.get(key) === request) {
        inflightRequests.delete(key)
      }
    })

  inflightRequests.set(key, request)
  return request
}

export function resetSharedRequests() {
  inflightRequests.clear()
  recentResults.clear()
}
