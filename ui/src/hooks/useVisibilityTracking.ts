import { useCallback, useEffect, useRef, useState } from 'react'

interface VisibilityBounds {
  top: number
  bottom: number
}

interface VisibilityItem {
  seq: number
  element: HTMLElement | null
}

function isRectVisibleWithinBounds(
  rect: Pick<DOMRect, 'top' | 'bottom'>,
  bounds: VisibilityBounds
): boolean {
  return rect.bottom > bounds.top && rect.top < bounds.bottom
}

export function getHighestVisibleSeq(items: VisibilityItem[], bounds: VisibilityBounds): number {
  let maxSeq = 0
  for (const item of items) {
    if (!item.element) continue
    const rect = item.element.getBoundingClientRect()
    if (isRectVisibleWithinBounds(rect, bounds) && item.seq > maxSeq) {
      maxSeq = item.seq
    }
  }
  return maxSeq
}

export function useVisibilityTracking(onHighestVisibleSeqChange?: (seq: number) => void) {
  const [highestVisibleSeq, setHighestVisibleSeq] = useState(0)
  const pendingReadsRef = useRef<VisibilityItem[]>([])
  const containerRef = useRef<HTMLElement | null>(null)
  const rafRef = useRef<number | null>(null)
  const onChangeRef = useRef(onHighestVisibleSeqChange)
  onChangeRef.current = onHighestVisibleSeqChange

  const flushVisibilityCheck = useCallback(() => {
    if (document.visibilityState !== 'visible') return

    const container = containerRef.current
    if (!container) return

    const bounds = container.getBoundingClientRect()
    const maxSeq = getHighestVisibleSeq(pendingReadsRef.current, {
      top: bounds.top,
      bottom: bounds.bottom,
    })

    if (maxSeq > 0) {
      setHighestVisibleSeq((current) => Math.max(current, maxSeq))
      onChangeRef.current?.(maxSeq)
    }

    pendingReadsRef.current = []
  }, [])

  const scheduleBatchVisibilityCheck = useCallback(
    (items: VisibilityItem[], container: HTMLElement | null) => {
      pendingReadsRef.current = items
      containerRef.current = container

      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current)
      }
      rafRef.current = requestAnimationFrame(flushVisibilityCheck)
    },
    [flushVisibilityCheck]
  )

  useEffect(() => {
    return () => {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current)
      }
    }
  }, [])

  return {
    highestVisibleSeq,
    scheduleBatchVisibilityCheck,
  }
}
