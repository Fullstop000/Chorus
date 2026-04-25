import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect } from 'vitest'
import { TaskCard } from './TaskCard'
import type { TaskInfo } from '../../data/tasks'

function task(partial: Partial<TaskInfo>): TaskInfo {
  return {
    id: 't-1',
    taskNumber: 7,
    title: 'wire up the bridge',
    status: 'todo',
    owner: null,
    createdBy: 'alice',
    createdAt: '2026-04-23T10:00:00Z',
    updatedAt: '2026-04-23T10:00:00Z',
    subChannelId: 'sub-1',
    subChannelName: 'eng__task-7',
    ...partial,
  }
}

describe('TaskCard', () => {
  it('renders accept and dismiss CTAs for proposed status', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({
          status: 'proposed',
          subChannelId: null,
          subChannelName: null,
          snapshotSenderName: 'bob',
          snapshotContent: 'we need to wire up the bridge',
        })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-testid="task-card-7"')
    expect(html).toContain('data-status="proposed"')
    expect(html).toContain('data-testid="task-card-accept-btn"')
    expect(html).toContain('data-testid="task-card-dismiss-btn"')
    expect(html).toContain('we need to wire up the bridge')
  })

  it('renders dismissed status as a muted card with no CTAs', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'dismissed' })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="dismissed"')
    expect(html).toContain('task-card--muted')
    expect(html).not.toContain('task-card-accept-btn')
    expect(html).not.toContain('task-card-claim-btn')
  })

  it('renders claim CTA when todo and unowned', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'todo', owner: null })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="todo"')
    expect(html).toContain('data-claimed="false"')
    expect(html).toContain('unowned')
    expect(html).toContain('data-testid="task-card-claim-btn"')
    expect(html).not.toContain('data-testid="task-card-start-btn"')
  })

  it('renders start CTA when todo and already owned', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'todo', owner: 'alice' })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="todo"')
    expect(html).toContain('data-claimed="true"')
    expect(html).toContain('claimed by @alice')
    expect(html).toContain('data-testid="task-card-start-btn"')
    expect(html).not.toContain('data-testid="task-card-claim-btn"')
  })

  it('renders sendForReview CTA + sub-channel link for in_progress', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'in_progress', owner: 'alice' })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="in_progress"')
    expect(html).toContain('@alice')
    expect(html).toContain('data-testid="task-card-review-btn"')
    expect(html).toContain('data-testid="task-card-link"')
  })

  it('renders markDone CTA + sub-channel link for in_review', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'in_review', owner: 'alice' })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="in_review"')
    expect(html).toContain('data-testid="task-card-done-btn"')
    expect(html).toContain('data-testid="task-card-link"')
  })

  it('renders a collapsed pill with strikethrough title for done', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'done', owner: 'alice' })}
        onAction={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="done"')
    expect(html).toContain('data-testid="task-card-done-pill"')
    expect(html).toContain('task-card__title--strike')
    expect(html).toContain('open →')
    // No CTAs in the done state — pill click is the only affordance.
    expect(html).not.toContain('task-card-review-btn')
    expect(html).not.toContain('task-card-done-btn')
  })

  it('disables every CTA when busy is true', () => {
    const html = renderToStaticMarkup(
      <TaskCard
        task={task({ status: 'todo', owner: null })}
        onAction={() => {}}
        busy={true}
      />,
    )
    // The claim button must be disabled while a request is in flight so users
    // can't double-fire it.
    expect(html).toMatch(/data-testid="task-card-claim-btn"[^>]*disabled/)
  })
})
