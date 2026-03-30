import { useCallback, useRef, useState } from "react"

interface VisibilityItem {
  seq: number
  id: string
  element: HTMLElement | null
}

export function useVisibilityTracking(getItemKey: (seq: number) => string) {
  const [highestVisibleSeq, setHighestVisibleSeq] = useState<number>(0)
  const pendingReadsRef = useRef<Map<string, number>>(new Map())
  const rafRef = useRef<number | null>(null)

  const collectHighestVisibleSeq = useCallback(() => {
    const items: VisibilityItem[] = []
    pendingReadsRef.current.forEach((seq, id) => {
      const element = document.getElementById(id)
      if (element) {
        items.push({ seq, id, element })
      }
    })

    let maxSeq = highestVisibleSeq
    for (const item of items) {
      const rect = item.element!.getBoundingClientRect()
      const isVisible = rect.top < window.innerHeight && rect.bottom > 0
      if (isVisible && item.seq > maxSeq) {
        maxSeq = item.seq
      }
    }

    if (maxSeq > highestVisibleSeq) {
      setHighestVisibleSeq(maxSeq)
    }

    pendingReadsRef.current.clear()
  }, [highestVisibleSeq])

  const scheduleVisibilityCheck = useCallback(
    (seq: number, id: string) => {
      pendingReadsRef.current.set(id, seq)

      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current)
      }
      rafRef.current = requestAnimationFrame(collectHighestVisibleSeq)
    },
    [collectHighestVisibleSeq]
  )

  const scheduleInitialVisibilityRead = useCallback(
    (seq: number) => {
      const id = getItemKey(seq)
      scheduleVisibilityCheck(seq, id)
    },
    [getItemKey, scheduleVisibilityCheck]
  )

  return {
    highestVisibleSeq,
    scheduleInitialVisibilityRead,
    scheduleVisibilityCheck,
  }
}
