import { useMemo } from 'react'
import type { HistoryMessage } from '../data/chat'
import { parseTaskEvent, type TaskEventPayload } from '../data/taskEvents'
import type { TaskStatus } from '../data/tasks'

export interface TaskEventRecord {
  eventId: string
  seq: number
  action: TaskEventPayload['action']
  actor: string
  prevStatus?: TaskStatus
  nextStatus: TaskStatus
  createdAt: string
}

export interface TaskState {
  taskNumber: number
  title: string
  subChannelId: string
  status: TaskStatus
  claimedBy: string | null
  /** Event history in seq order, oldest first. */
  events: TaskEventRecord[]
  /** seq of the latest event applied — used for repeat-suppression. */
  latestSeq: number
}

export interface TaskEventIndex {
  /** Per-task state, keyed by taskNumber. */
  byTaskNumber: Map<number, TaskState>
  /** Inverse index: seq → taskNumber. Lets the render loop detect a
   *  task_event row in O(1) without re-parsing JSON content. */
  taskNumberBySeq: Map<number, number>
}

/**
 * Reduce a message stream into per-task state + a seq→taskNumber inverse
 * index. Pure function: same input yields same output. Non-task_event
 * messages are ignored. Out-of-order arrivals are tolerated — events are
 * applied in seq order.
 *
 * Internally builds each task's `events` list with in-place `push`, not
 * spread-copy, so the cost is O(n) over n total events, not O(k²) over
 * k events per task.
 */
export function deriveTaskStates(messages: HistoryMessage[]): TaskEventIndex {
  const parsed: { msg: HistoryMessage; ev: TaskEventPayload }[] = []
  for (const msg of messages) {
    if (msg.senderType !== 'system') continue
    const ev = parseTaskEvent(msg.payload)
    if (!ev) continue
    parsed.push({ msg, ev })
  }
  parsed.sort((a, b) => a.msg.seq - b.msg.seq)

  const byTaskNumber = new Map<number, TaskState>()
  const taskNumberBySeq = new Map<number, number>()

  for (const { msg, ev } of parsed) {
    taskNumberBySeq.set(msg.seq, ev.taskNumber)

    const record: TaskEventRecord = {
      eventId: msg.id,
      seq: msg.seq,
      action: ev.action,
      actor: ev.actor,
      prevStatus: ev.prevStatus,
      nextStatus: ev.nextStatus,
      createdAt: msg.createdAt,
    }

    const prev = byTaskNumber.get(ev.taskNumber)
    if (!prev) {
      byTaskNumber.set(ev.taskNumber, {
        taskNumber: ev.taskNumber,
        title: ev.title,
        subChannelId: ev.subChannelId,
        status: ev.nextStatus,
        claimedBy: ev.claimedBy ?? null,
        events: [record],
        latestSeq: msg.seq,
      })
    } else {
      // Mutation is safe: `prev` is a local object owned by this call, and
      // the returned Map is built fresh on every memo recompute.
      prev.title = ev.title
      prev.subChannelId = ev.subChannelId
      prev.status = ev.nextStatus
      if (ev.claimedBy !== undefined) {
        prev.claimedBy = ev.claimedBy ?? null
      }
      prev.events.push(record)
      prev.latestSeq = msg.seq
    }
  }
  return { byTaskNumber, taskNumberBySeq }
}

/** React hook wrapping `deriveTaskStates` with memoization. */
export function useTaskEventLog(messages: HistoryMessage[]): TaskEventIndex {
  return useMemo(() => deriveTaskStates(messages), [messages])
}
