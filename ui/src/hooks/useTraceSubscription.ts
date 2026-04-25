import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { agentQueryKeys } from '../data'
import { getSession } from '../transport/session'
import { useTraceStore } from '../store/traceStore'

/** Subscribe to agent trace frames and push them into the trace store. */
export function useTraceSubscription(viewer: string | null) {
  const pushEvent = useTraceStore((s) => s.pushEvent)
  const queryClient = useQueryClient()

  useEffect(() => {
    if (!viewer) return
    const session = getSession(viewer)
    let refreshTimer: number | null = null
    const unsubscribe = session.subscribeTraces((frame) => {
      pushEvent(frame)
      if (refreshTimer != null) return
      refreshTimer = window.setTimeout(() => {
        refreshTimer = null
        void queryClient.invalidateQueries({ queryKey: agentQueryKeys.agents })
      }, 100)
    })

    return () => {
      unsubscribe()
      if (refreshTimer != null) window.clearTimeout(refreshTimer)
    }
  }, [viewer, pushEvent, queryClient])
}
