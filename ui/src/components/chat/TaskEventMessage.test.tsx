import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect } from 'vitest'
import { TaskEventMessage } from './TaskEventMessage'
import type { TaskState } from '../../hooks/useTaskEventLog'

function state(partial: Partial<TaskState>): TaskState {
  return {
    taskNumber: 7,
    title: 'wire up the bridge',
    subChannelId: 'sub-1',
    status: 'todo',
    claimedBy: null,
    events: [
      {
        eventId: 'e1',
        seq: 1,
        action: 'created',
        actor: 'alice',
        nextStatus: 'todo',
        createdAt: '2026-04-23T10:00:00Z',
      },
    ],
    latestSeq: 1,
    ...partial,
  }
}

describe('TaskEventMessage', () => {
  it('renders the living card when status is not done', () => {
    const html = renderToStaticMarkup(
      <TaskEventMessage taskState={state({ status: 'in_progress', claimedBy: 'alice' })} onOpen={() => {}} />,
    )
    expect(html).toContain('wire up the bridge')
    expect(html).toContain('#7')
    expect(html).toContain('claimed by alice')
    expect(html).toContain('data-state="in_progress"')
    expect(html).toContain('data-status="in_progress"')
    expect(html).toContain('task-event-thread')
  })

  it('renders the done pill when status is done', () => {
    const html = renderToStaticMarkup(
      <TaskEventMessage taskState={state({ status: 'done', claimedBy: 'alice' })} onOpen={() => {}} />,
    )
    expect(html).toContain('data-state="done"')
    // The card-view is still rendered in markup (CSS collapses it) — we just
    // verify the pill-view contents exist.
    expect(html).toContain('task-event-done-row')
  })

  it('renders one timeline event per history entry, in order', () => {
    const html = renderToStaticMarkup(
      <TaskEventMessage
        taskState={state({
          status: 'in_review',
          claimedBy: 'alice',
          events: [
            { eventId: 'e1', seq: 1, action: 'created', actor: 'alice', nextStatus: 'todo', createdAt: '2026-04-23T10:00:00Z' },
            { eventId: 'e2', seq: 2, action: 'claimed', actor: 'alice', nextStatus: 'in_progress', createdAt: '2026-04-23T10:05:00Z' },
            { eventId: 'e3', seq: 3, action: 'status_changed', actor: 'alice', prevStatus: 'in_progress', nextStatus: 'in_review', createdAt: '2026-04-23T11:00:00Z' },
          ],
        })}
        onOpen={() => {}}
      />,
    )
    expect(html).toContain('alice claimed')
    expect(html).toContain('in review')
  })
})
