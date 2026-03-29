import { createRealtimeSocket } from './realtime'
import type { RealtimeMessage } from '../types'

interface RealtimeSubscription {
  targets: string[]
  onFrame: (frame: RealtimeMessage) => void
}

interface SubscriptionState extends RealtimeSubscription {
  id: string
}

function logRealtime(event: string, detail: unknown) {
  let rendered = String(detail)
  if (typeof detail === 'string') {
    rendered = detail
  } else {
    try {
      rendered = JSON.stringify(detail)
    } catch {
      rendered = String(detail)
    }
  }
  console.debug(`[chorus:realtime] ${event} ${rendered}`)
}

class RealtimeSession {
  private socket: WebSocket | null = null
  private reconnectTimer: number | null = null
  private disposed = false
  private nextSubscriptionId = 0
  private subscriptions = new Map<string, SubscriptionState>()

  constructor(private readonly viewer: string) {}

  subscribe(subscription: RealtimeSubscription): () => void {
    const id = `rt-sub-${this.nextSubscriptionId++}`
    this.subscriptions.set(id, { ...subscription, id })
    this.ensureSocket()
    this.syncSubscriptions()
    return () => {
      if (!this.subscriptions.delete(id)) return
      this.syncSubscriptions()
    }
  }

  dispose() {
    this.disposed = true
    this.subscriptions.clear()
    if (this.reconnectTimer != null) {
      window.clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.socket?.close()
    this.socket = null
  }

  private ensureSocket() {
    if (this.socket || this.disposed) return
    const socket = createRealtimeSocket(this.viewer)
    this.socket = socket

    socket.onopen = () => {
      logRealtime('open', {
        viewer: this.viewer,
        targets: this.currentTargets(),
      })
      this.syncSubscriptions()
    }

    socket.onmessage = (messageEvent) => {
      try {
        const frame = JSON.parse(String(messageEvent.data)) as RealtimeMessage
        logRealtime('recv', frame)
        for (const subscription of this.subscriptions.values()) {
          if (!this.subscriptionMatchesFrame(subscription, frame)) {
            continue
          }
          subscription.onFrame(frame)
        }
      } catch (eventError) {
        console.error('Failed to parse realtime frame', eventError)
      }
    }

    socket.onerror = (event) => {
      logRealtime('error', event)
      socket.close()
    }

    socket.onclose = () => {
      logRealtime('close', {
        viewer: this.viewer,
        targets: this.currentTargets(),
      })
      if (this.socket === socket) {
        this.socket = null
      }
      if (this.disposed) return
      if (this.reconnectTimer != null) {
        window.clearTimeout(this.reconnectTimer)
      }
      this.reconnectTimer = window.setTimeout(() => {
        this.reconnectTimer = null
        this.ensureSocket()
      }, 1_000)
    }
  }

  private currentTargets(): string[] {
    return [
      ...new Set(
        [...this.subscriptions.values()].flatMap((subscription) => subscription.targets)
      ),
    ]
      .sort()
  }

  private eventTargets(frame: RealtimeMessage): string[] {
    if (frame.type !== 'event') return []
    const targets = new Set<string>()
    if (frame.event.channelId) {
      targets.add(`conversation:${frame.event.channelId}`)
    }
    const threadParentId = frame.event.payload.threadParentId
    if (typeof threadParentId === 'string') {
      targets.add(`thread:${threadParentId}`)
    }
    return [...targets]
  }

  private subscriptionMatchesFrame(subscription: SubscriptionState, frame: RealtimeMessage): boolean {
    if (frame.type !== 'event') return true
    const eventTargets = this.eventTargets(frame)
    if (eventTargets.length === 0) return true
    return subscription.targets.some((target) => eventTargets.includes(target))
  }

  private syncSubscriptions() {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return

    const targets = this.currentTargets()
    const frame: Record<string, unknown> = {
      type: 'subscribe',
      replace: true,
      targets,
    }

    logRealtime('send', frame)
    this.socket.send(JSON.stringify(frame))
  }
}

const realtimeSessions = new Map<string, RealtimeSession>()

export function getRealtimeSession(viewer: string): RealtimeSession {
  let session = realtimeSessions.get(viewer)
  if (!session) {
    session = new RealtimeSession(viewer)
    realtimeSessions.set(viewer, session)
  }
  return session
}

export function resetRealtimeSessions() {
  for (const session of realtimeSessions.values()) {
    session.dispose()
  }
  realtimeSessions.clear()
}
