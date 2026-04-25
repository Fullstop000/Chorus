import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect } from 'vitest'
import { TaskEventRow } from './TaskEventRow'
import type { TaskEventPayload } from '../../data/taskEvents'

function payload(partial: Partial<TaskEventPayload>): TaskEventPayload {
  return {
    kind: 'task_event',
    action: 'claimed',
    taskNumber: 7,
    title: 'wire up the bridge',
    subChannelId: 'sub-1',
    actor: 'alice',
    nextStatus: 'in_progress',
    claimedBy: 'alice',
    ...partial,
  }
}

describe('TaskEventRow', () => {
  it('formats a claimed event with the actor handle', () => {
    const html = renderToStaticMarkup(
      <TaskEventRow
        event={payload({ action: 'claimed', actor: 'alice' })}
        eventId="m1"
        createdAt="2026-04-23T10:00:00Z"
        seq={1}
      />,
    )
    expect(html).toContain('@alice claimed')
    expect(html).toContain('data-action="claimed"')
    expect(html).toContain('#7')
  })

  it('formats an unclaimed event', () => {
    const html = renderToStaticMarkup(
      <TaskEventRow
        event={payload({ action: 'unclaimed', actor: 'bob', claimedBy: null })}
        eventId="m2"
        createdAt="2026-04-23T10:05:00Z"
        seq={2}
      />,
    )
    expect(html).toContain('@bob unclaimed')
    expect(html).toContain('data-action="unclaimed"')
  })

  it('formats a status_changed event with the next status, space-separated', () => {
    const html = renderToStaticMarkup(
      <TaskEventRow
        event={payload({
          action: 'status_changed',
          prevStatus: 'in_progress',
          nextStatus: 'in_review',
        })}
        eventId="m3"
        createdAt="2026-04-23T11:00:00Z"
        seq={3}
      />,
    )
    expect(html).toContain('in review')
    expect(html).not.toContain('in_review')
  })
})
