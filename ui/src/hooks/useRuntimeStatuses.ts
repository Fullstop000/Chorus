import { useEffect, useState } from 'react'
import { listRuntimeStatuses } from '../data'
import type { RuntimeStatusInfo } from '../data'

export function useRuntimeStatuses(pollMs = 10000): {
  runtimeStatuses: RuntimeStatusInfo[]
  runtimeStatusError: string | null
} {
  const [runtimeStatuses, setRuntimeStatuses] = useState<RuntimeStatusInfo[]>([])
  const [runtimeStatusError, setRuntimeStatusError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false

    async function refreshRuntimeStatuses() {
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

    void refreshRuntimeStatuses()
    const intervalId = window.setInterval(() => {
      void refreshRuntimeStatuses()
    }, pollMs)

    return () => {
      cancelled = true
      window.clearInterval(intervalId)
    }
  }, [pollMs])

  return { runtimeStatuses, runtimeStatusError }
}
