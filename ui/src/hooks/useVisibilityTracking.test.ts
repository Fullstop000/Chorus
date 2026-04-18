import { describe, expect, it } from 'vitest'
import { getHighestVisibleSeq } from './useVisibilityTracking'

function createElement(top: number, bottom: number): HTMLElement {
  return {
    getBoundingClientRect: () => ({ top, bottom }),
  } as unknown as HTMLElement
}

describe('getHighestVisibleSeq', () => {
  it('only counts rows visible inside the panel bounds', () => {
    const highestVisibleSeq = getHighestVisibleSeq(
      [
        { seq: 3, element: createElement(80, 130) },
        { seq: 4, element: createElement(340, 390) },
      ],
      { top: 100, bottom: 300 }
    )

    expect(highestVisibleSeq).toBe(3)
  })

  it('ignores missing elements', () => {
    const highestVisibleSeq = getHighestVisibleSeq(
      [
        { seq: 2, element: null },
        { seq: 5, element: createElement(120, 180) },
      ],
      { top: 100, bottom: 300 }
    )

    expect(highestVisibleSeq).toBe(5)
  })
})
