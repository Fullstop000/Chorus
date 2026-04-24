import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect } from 'vitest'
import { TaskProposalMessage } from './TaskProposalMessage'
import type { TaskProposalState } from '../../hooks/useTaskProposalLog'

function pending(
  overrides: Partial<TaskProposalState> = {},
): TaskProposalState {
  return {
    proposalId: 'p1',
    status: 'pending',
    title: 'fix login',
    proposedBy: 'claude',
    proposedAt: '2026-04-23T10:00:00Z',
    taskNumber: null,
    subChannelId: null,
    subChannelName: null,
    resolvedBy: null,
    resolvedAt: null,
    sourceMessageId: null,
    snapshotSenderName: null,
    snapshotExcerpt: null,
    snapshotCreatedAt: null,
    latestSeq: 1,
    ...overrides,
  }
}

describe('TaskProposalMessage', () => {
  it('renders pending state with create + dismiss buttons', () => {
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={pending()}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('create')
    expect(html).toContain('dismiss')
    expect(html).toContain('fix login')
    expect(html).toContain('claude')
    expect(html).toContain('data-testid="task-proposal-p1"')
    expect(html).toContain('data-status="pending"')
  })

  it('renders accepted state with task coords + open link', () => {
    const state: TaskProposalState = {
      ...pending(),
      status: 'accepted',
      taskNumber: 7,
      subChannelId: 's',
      subChannelName: 'eng__task-7',
      resolvedBy: 'alice',
      latestSeq: 2,
    }
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={state}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="accepted"')
    expect(html).toContain('#7')
    expect(html).toContain('eng__task-7')
    expect(html).not.toContain('data-testid="task-proposal-accept-btn"')
  })

  it('renders dismissed state without action buttons', () => {
    const state: TaskProposalState = {
      ...pending(),
      status: 'dismissed',
      resolvedBy: 'alice',
      latestSeq: 2,
    }
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={state}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('data-status="dismissed"')
    expect(html).not.toContain('data-testid="task-proposal-accept-btn"')
    expect(html).not.toContain('data-testid="task-proposal-dismiss-btn"')
  })

  it('renders snapshot excerpt on pending when present', () => {
    const state = pending({
      snapshotSenderName: 'alice',
      snapshotExcerpt: 'login breaks on Safari',
    })
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={state}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    expect(html).toContain('task-proposal__excerpt')
    expect(html).toContain('@alice')
    expect(html).toContain('login breaks on Safari')
  })

  it('omits excerpt block when snapshotExcerpt is null', () => {
    const state = pending({
      snapshotSenderName: null,
      snapshotExcerpt: null,
    })
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={state}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    expect(html).not.toContain('task-proposal__excerpt')
  })

  it('renders excerpt as plain text, not markdown', () => {
    const state = pending({
      snapshotSenderName: 'alice',
      snapshotExcerpt: '```rust\nfn foo() {}\n```',
    })
    const html = renderToStaticMarkup(
      <TaskProposalMessage
        state={state}
        onAccept={() => {}}
        onDismiss={() => {}}
        busy={false}
      />,
    )
    // Triple backticks render verbatim — no code block conversion.
    expect(html).toContain('```rust')
    // No markdown rendering means no generated <pre>/<code> inside excerpt.
    expect(html).not.toContain('<pre')
    expect(html).not.toContain('<code')
  })
})
