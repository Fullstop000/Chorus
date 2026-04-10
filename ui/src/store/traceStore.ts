import { create } from 'zustand'
import type { TraceFrame } from '../transport/types'

export interface AgentTrace {
  runId: string
  events: TraceFrame[]
  isActive: boolean
  isError: boolean
}

// ── Completion sound (Delight Touch 7) ──

const SOUND_THRESHOLD_MS = Number(localStorage.getItem('TELESCOPE_SOUND_THRESHOLD_MS') ?? 30000)

function isSoundEnabled(): boolean {
  return localStorage.getItem('TELESCOPE_SOUND_ENABLED') === 'true'
}

function playCompletionChime() {
  try {
    const ctx = new AudioContext()
    const osc = ctx.createOscillator()
    const gain = ctx.createGain()
    osc.connect(gain)
    gain.connect(ctx.destination)
    osc.type = 'sine'
    osc.frequency.setValueAtTime(880, ctx.currentTime)
    osc.frequency.setValueAtTime(1100, ctx.currentTime + 0.08)
    gain.gain.setValueAtTime(0.08, ctx.currentTime)
    gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.3)
    osc.start(ctx.currentTime)
    osc.stop(ctx.currentTime + 0.3)
  } catch { /* audio context unavailable */ }
}

interface TraceState {
  /** Per-agent trace state, keyed by agent name. */
  traces: Record<string, AgentTrace>
  /** Push a trace event from the WebSocket. */
  pushEvent: (frame: TraceFrame) => void
  /** Toggle expanded/collapsed for an agent's Telescope. */
  expandedAgents: Record<string, boolean>
  toggleExpanded: (agentName: string) => void
  /** Agents whose Telescope header should flash green on completion. */
  completionFlash: Record<string, boolean>
}

export const useTraceStore = create<TraceState>((set) => ({
  traces: {},
  expandedAgents: {},
  completionFlash: {},

  pushEvent: (frame) =>
    set((state) => {
      const prev = state.traces[frame.agentName]
      const isNewRun = !prev || prev.runId !== frame.runId
      const events = isNewRun ? [frame] : [...prev.events, frame]
      const isError = frame.kind === 'error'
      const isActive = frame.kind !== 'turn_end' && !isError
      const isRunEnd = frame.kind === 'turn_end' || frame.kind === 'error'

      // Completion sound + flash
      let flash = state.completionFlash
      if (isRunEnd && !isError && events.length >= 2) {
        const startMs = events[0].timestampMs
        const endMs = frame.timestampMs
        if (endMs - startMs >= SOUND_THRESHOLD_MS) {
          if (isSoundEnabled()) playCompletionChime()
          flash = { ...flash, [frame.agentName]: true }
          setTimeout(() => {
            useTraceStore.setState((s) => ({
              completionFlash: { ...s.completionFlash, [frame.agentName]: false },
            }))
          }, 1200)
        }
      }

      return {
        traces: {
          ...state.traces,
          [frame.agentName]: {
            runId: frame.runId,
            events,
            isActive,
            isError,
          },
        },
        completionFlash: flash,
      }
    }),

  toggleExpanded: (agentName) =>
    set((state) => ({
      expandedAgents: {
        ...state.expandedAgents,
        [agentName]: !(state.expandedAgents[agentName] ?? true),
      },
    })),
}))
