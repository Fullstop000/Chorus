import { useEffect } from 'react'
import { getSession } from '../transport/session'
import { useTraceStore } from '../store/traceStore'

/** Subscribe to agent trace frames and push them into the trace store. */
export function useTraceSubscription(viewer: string | null) {
  const pushEvent = useTraceStore((s) => s.pushEvent)

  useEffect(() => {
    if (!viewer) return
    const session = getSession(viewer)
    return session.subscribeTraces(pushEvent)
  }, [viewer, pushEvent])
}
