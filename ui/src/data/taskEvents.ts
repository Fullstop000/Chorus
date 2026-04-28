import type { TaskStatus } from './tasks'
import type { MessagePayload } from './chat'

export type TaskEventAction =
  | 'created'
  | 'claimed'
  | 'unclaimed'
  | 'status_changed'

export interface TaskEventPayload {
  kind: 'task_event'
  action: TaskEventAction
  taskNumber: number
  title: string
  subChannelId: string
  actor: string
  prevStatus?: TaskStatus
  nextStatus: TaskStatus
  claimedBy?: string | null
}

const VALID_ACTIONS: readonly TaskEventAction[] = [
  'created',
  'claimed',
  'unclaimed',
  'status_changed',
]
const VALID_STATUSES: readonly TaskStatus[] = [
  'todo',
  'in_progress',
  'in_review',
  'done',
]

/**
 * Narrow a generic message `payload` object to a TaskEventPayload. Returns
 * null for anything that isn't a well-formed task_event payload. Never throws.
 *
 * Called for every system-sender message in the chat feed. Performance matters.
 */
export function parseTaskEvent(
  payload: MessagePayload | undefined,
): TaskEventPayload | null {
  if (!payload || payload.kind !== 'task_event') return null
  const obj = payload as Record<string, unknown>
  if (typeof obj.action !== 'string' || !VALID_ACTIONS.includes(obj.action as TaskEventAction)) {
    return null
  }
  // taskNumber must be a finite integer. Strict TS won't narrow `unknown` to
  // `number` via `Number.isInteger` alone, so check both: the typeof first
  // narrows the type, then Number.isInteger rejects floats (`typeof 1.5 ===
  // 'number'` so a pure typeof check would let them through).
  if (typeof obj.taskNumber !== 'number' || !Number.isInteger(obj.taskNumber)) {
    return null
  }
  if (typeof obj.title !== 'string') return null
  if (typeof obj.subChannelId !== 'string') return null
  if (typeof obj.actor !== 'string') return null
  if (typeof obj.nextStatus !== 'string' || !VALID_STATUSES.includes(obj.nextStatus as TaskStatus)) {
    return null
  }
  // prevStatus is optional, but when present it MUST be a valid status — a
  // malformed value means the producer is broken, and silently dropping it
  // weakens the "parseTaskEvent returns null on anything wrong" contract.
  let prevStatus: TaskStatus | undefined
  if (obj.prevStatus === undefined || obj.prevStatus === null) {
    prevStatus = undefined
  } else if (
    typeof obj.prevStatus === 'string' &&
    VALID_STATUSES.includes(obj.prevStatus as TaskStatus)
  ) {
    prevStatus = obj.prevStatus as TaskStatus
  } else {
    return null
  }
  // claimedBy: absent / null / string. A wrong-type value (number, object,
  // array) means the producer is broken — fail the parse instead of silently
  // coercing to undefined, which would paint the card as unclaimed.
  let claimedBy: string | null | undefined
  if (obj.claimedBy === undefined) {
    claimedBy = undefined
  } else if (obj.claimedBy === null) {
    claimedBy = null
  } else if (typeof obj.claimedBy === 'string') {
    claimedBy = obj.claimedBy
  } else {
    return null
  }
  return {
    kind: 'task_event',
    action: obj.action as TaskEventAction,
    taskNumber: obj.taskNumber,
    title: obj.title,
    subChannelId: obj.subChannelId,
    actor: obj.actor,
    prevStatus,
    nextStatus: obj.nextStatus as TaskStatus,
    claimedBy,
  }
}
