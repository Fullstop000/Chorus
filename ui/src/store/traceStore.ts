import { create } from 'zustand'
import type { TraceFrame } from '../transport/types'

export interface AgentTrace {
  runId: string
  events: TraceFrame[]
  isActive: boolean
  isError: boolean
}

interface TraceState {
  /** Per-agent trace state, keyed by agent name. */
  traces: Record<string, AgentTrace>
  /** Push a trace event from the WebSocket. */
  pushEvent: (frame: TraceFrame) => void
  /** Toggle expanded/collapsed for an agent's Telescope. */
  expandedAgents: Record<string, boolean>
  toggleExpanded: (agentName: string) => void
}

export const useTraceStore = create<TraceState>((set) => ({
  traces: {},
  expandedAgents: {},

  pushEvent: (frame) =>
    set((state) => {
      const prev = state.traces[frame.agentName]
      const isNewRun = !prev || prev.runId !== frame.runId
      const events = isNewRun ? [frame] : [...prev.events, frame]
      const isError = frame.kind === 'error'
      const isActive = frame.kind !== 'turn_end' && !isError

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
