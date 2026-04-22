import { useEffect, useState } from 'react'
import { listRuntimeStatuses } from '../data'
import type { RuntimeCatalogEntry } from '../data'

export function useRuntimeStatuses(enabled = true): {
  runtimeStatuses: RuntimeCatalogEntry[]
  runtimeStatusError: string | null
} {
  const [runtimeStatuses, setRuntimeStatuses] = useState<RuntimeCatalogEntry[]>([])
  const [runtimeStatusError, setRuntimeStatusError] = useState<string | null>(null)

  useEffect(() => {
    if (!enabled) return
    let cancelled = false

    async function fetchRuntimeStatuses() {
      try {
        const nextStatuses = await listRuntimeStatuses()
        if (!cancelled) {
          setRuntimeStatuses(nextStatuses)
          setRuntimeStatusError(null)
        }
      } catch (err) {
        if (!cancelled) {
          setRuntimeStatusError(String(err))
        }
      }
    }

    void fetchRuntimeStatuses()

    return () => {
      cancelled = true
    }
  }, [enabled])

  return { runtimeStatuses, runtimeStatusError }
}
