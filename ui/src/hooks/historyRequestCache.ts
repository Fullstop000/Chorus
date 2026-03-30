const inflightRequests = new Map<string, Promise<unknown>>()

export function loadSharedRequest<T>(key: string, loader: () => Promise<T>): Promise<T> {
  const inflight = inflightRequests.get(key)
  if (inflight) {
    return inflight as Promise<T>
  }

  const request = loader()
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
}
