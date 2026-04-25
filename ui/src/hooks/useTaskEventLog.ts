import { useMemo } from 'react'
import type { HistoryMessage } from '../data/chat'
import { parseTaskEvent, type TaskEventPayload } from '../data/taskEvents'

/**
 * One row per parsed task_event message. Rendered inline by `TaskEventRow`
 * inside the sub-channel. Pre-T9 this hook reduced events into per-task
 * card state — that role moves to the parent-channel `TaskCard`, which
 * subscribes to the tasks store directly. Sub-channel renderers only need a
 * flat, ordered log.
 */
export interface TaskEventRecord {
  /** Source message id — stable React key. */
  eventId: string
  seq: number
  createdAt: string
  payload: TaskEventPayload
}

/**
 * Reduce a message stream to an ordered list of task_event rows. Pure
 * function: same input yields same output. Non-task_event messages are
 * ignored. Out-of-order arrivals are tolerated — entries are sorted by seq
 * before return.
 */
export function deriveTaskEventRows(messages: HistoryMessage[]): TaskEventRecord[] {
  const rows: TaskEventRecord[] = []
  for (const msg of messages) {
    if (msg.senderType !== 'system') continue
    const payload = parseTaskEvent(msg.content)
    if (!payload) continue
    rows.push({
      eventId: msg.id,
      seq: msg.seq,
      createdAt: msg.createdAt,
      payload,
    })
  }
  rows.sort((a, b) => a.seq - b.seq)
  return rows
}

/** React hook wrapping `deriveTaskEventRows` with memoization. */
export function useTaskEventLog(messages: HistoryMessage[]): TaskEventRecord[] {
  return useMemo(() => deriveTaskEventRows(messages), [messages])
}
