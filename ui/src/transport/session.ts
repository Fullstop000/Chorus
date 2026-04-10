// ── Realtime transport session ──
// WebSocket lifecycle, subscriber registry, frame dispatch.
// No app logic — consumers decide what events mean.

import type { RealtimeFrame, TraceFrame } from './types'

// ── Subscriber entry stored in the registry ──

interface Subscriber {
  id: string
  channelId: string | null  // null = wildcard (receives all events)
  onEvent: (frame: RealtimeFrame) => void
}

// ── Session: one WebSocket per viewer, many subscribers ──

let nextSubId = 0

export class RealtimeSession {
  private socket: WebSocket | null = null
  private reconnectTimer: number | null = null
  private disposed = false
  private subscribers = new Map<string, Subscriber>()
  private traceSubscribers = new Map<string, (frame: TraceFrame) => void>()

  constructor(private readonly viewer: string) {}

  /** Subscribe to events for a specific channel. Returns unsubscribe fn. */
  subscribe(channelId: string, onEvent: (frame: RealtimeFrame) => void): () => void {
    return this.addSubscriber(channelId, onEvent)
  }

  /** Subscribe to all events (wildcard). Returns unsubscribe fn. */
  subscribeAll(onEvent: (frame: RealtimeFrame) => void): () => void {
    return this.addSubscriber(null, onEvent)
  }

  /** Subscribe to agent trace frames only. Returns unsubscribe fn. */
  subscribeTraces(onTrace: (frame: TraceFrame) => void): () => void {
    const id = `trace-${nextSubId++}`
    this.traceSubscribers.set(id, onTrace)
    this.ensureSocket()
    return () => { this.traceSubscribers.delete(id) }
  }

  dispose() {
    this.disposed = true
    this.subscribers.clear()
    this.traceSubscribers.clear()
    if (this.reconnectTimer != null) {
      window.clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.socket?.close()
    this.socket = null
  }

  private addSubscriber(
    channelId: string | null,
    onEvent: (frame: RealtimeFrame) => void
  ): () => void {
    const id = `sub-${nextSubId++}`
    this.subscribers.set(id, { id, channelId, onEvent })
    this.ensureSocket()
    return () => { this.subscribers.delete(id) }
  }

  private ensureSocket() {
    if (this.socket || this.disposed) return

    const url = new URL('/api/events/ws', window.location.origin)
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    url.searchParams.set('viewer', this.viewer)
    const socket = new WebSocket(url)
    this.socket = socket

    socket.onopen = () => {
      console.debug('[chorus:realtime] open', this.viewer)
    }

    // ── Frame dispatch: parse once, fan out to matching subscribers ──
    // Wildcard subscribers (channelId=null) receive every frame.
    // Channel subscribers only receive frames whose event.channelId matches.
    // Error frames go to all subscribers — they may need to react.
    socket.onmessage = (raw) => {
      let frame: RealtimeFrame
      try {
        frame = JSON.parse(String(raw.data)) as RealtimeFrame
      } catch {
        console.error('[chorus:realtime] bad frame', raw.data)
        return
      }

      // Route trace frames to dedicated trace subscribers only.
      if (frame.type === 'trace') {
        for (const cb of this.traceSubscribers.values()) {
          cb(frame.event)
        }
        return
      }

      const frameChannelId = frame.type === 'event' ? frame.event.channelId : null

      for (const sub of this.subscribers.values()) {
        if (sub.channelId !== null && frameChannelId !== null && sub.channelId !== frameChannelId) {
          continue
        }
        sub.onEvent(frame)
      }
    }

    socket.onerror = () => {
      console.debug('[chorus:realtime] error', this.viewer)
      socket.close()
    }

    // ── Reconnect: 1s backoff, runs until disposed or session GC'd ──
    // The socket is nulled first so ensureSocket() will create a fresh one.
    // If disposed, we bail — no zombie reconnects.
    socket.onclose = () => {
      console.debug('[chorus:realtime] close', this.viewer)
      if (this.socket === socket) this.socket = null
      if (this.disposed) return
      if (this.reconnectTimer != null) window.clearTimeout(this.reconnectTimer)
      this.reconnectTimer = window.setTimeout(() => {
        this.reconnectTimer = null
        this.ensureSocket()
      }, 1_000)
    }
  }
}

// ── Singleton session management ──

const sessions = new Map<string, RealtimeSession>()

export function getSession(viewer: string): RealtimeSession {
  let session = sessions.get(viewer)
  if (!session) {
    session = new RealtimeSession(viewer)
    sessions.set(viewer, session)
  }
  return session
}

export function resetSessions() {
  for (const session of sessions.values()) session.dispose()
  sessions.clear()
}
