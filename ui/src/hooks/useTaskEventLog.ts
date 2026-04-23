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

/**
 * Reduce a message stream into per-task state. Pure function: same input
 * yields same output. Messages that aren't task_event system messages are
 * ignored. Out-of-order arrivals are tolerated — events are applied in seq
 * order.
 */
export function deriveTaskStates(messages: HistoryMessage[]): Map<number, TaskState> {
  const parsed: { msg: HistoryMessage; ev: TaskEventPayload }[] = []
  for (const msg of messages) {
    if (msg.senderType !== 'system') continue
    const ev = parseTaskEvent(msg.content)
    if (!ev) continue
    parsed.push({ msg, ev })
  }
  parsed.sort((a, b) => a.msg.seq - b.msg.seq)

  const out = new Map<number, TaskState>()
  for (const { msg, ev } of parsed) {
    const record: TaskEventRecord = {
      eventId: msg.id,
      seq: msg.seq,
      action: ev.action,
      actor: ev.actor,
      prevStatus: ev.prevStatus,
      nextStatus: ev.nextStatus,
      createdAt: msg.createdAt,
    }
    const prev = out.get(ev.taskNumber)
    if (!prev) {
      out.set(ev.taskNumber, {
        taskNumber: ev.taskNumber,
        title: ev.title,
        subChannelId: ev.subChannelId,
        status: ev.nextStatus,
        claimedBy: ev.claimedBy ?? null,
        events: [record],
        latestSeq: msg.seq,
      })
    } else {
      out.set(ev.taskNumber, {
        ...prev,
        title: ev.title,
        subChannelId: ev.subChannelId,
        status: ev.nextStatus,
        claimedBy: ev.claimedBy === undefined ? prev.claimedBy : (ev.claimedBy ?? null),
        events: [...prev.events, record],
        latestSeq: msg.seq,
      })
    }
  }
  return out
}

/** React hook wrapping `deriveTaskStates` with memoization. */
export function useTaskEventLog(messages: HistoryMessage[]): Map<number, TaskState> {
  return useMemo(() => deriveTaskStates(messages), [messages])
}
