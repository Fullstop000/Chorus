import { useRef, useEffect, useState, useCallback } from "react";
import { classifyTool, iconForCategory } from "../../lib/toolCategories";
import { getTraceEvents } from "../../data/chat";
import { useTraceStore } from "../../store/traceStore";
import type { TraceSummary, TraceEventRecord } from "../../data/chat";
import "./Telescope.css";

// ── Trace event types (canonical source: transport/types.ts) ──

export interface TraceEvent {
  runId: string;
  agentName: string;
  seq: number;
  timestampMs: number;
  kind: string;
  data: Record<string, string>;
}

// ── Props ──

interface TelescopeProps {
  agentName: string;
  runId?: string;
  events: TraceEvent[];
  isActive: boolean;
  isError: boolean;
  onToggleExpand?: () => void;
  isExpanded?: boolean;
  traceSummary?: TraceSummary;
}

// ── Helpers ──

function truncate(s: string, max: number): string {
  if (s.length <= max) return s;
  return s.slice(0, max) + "…";
}

/** Derive the agent's current phase from live trace events. */
type AgentPhase = "reading" | "thinking" | "doing" | "responding";

function derivePhase(events: TraceEvent[]): AgentPhase {
  if (events.length === 0) return "reading";
  for (let i = events.length - 1; i >= 0; i--) {
    const k = events[i].kind;
    if (k === "tool_call" || k === "tool_done") return "doing";
  }
  if (events.some((e) => e.kind === "thinking")) return "thinking";
  if (events.some((e) => e.kind === "text")) return "responding";
  return "reading";
}

// ── Tool category chips ──

function deriveCategories(events: TraceEvent[]): Record<string, number> {
  const cats: Record<string, number> = {};
  for (const e of events) {
    if (e.kind === "tool_call" || e.kind === "tool_done") {
      const name = getToolName(e.data);
      const { category } = classifyTool(name);
      cats[category] = (cats[category] ?? 0) + 1;
    }
  }
  return cats;
}

function CategoryChips({
  categories,
  duration,
}: {
  categories: Record<string, number>;
  duration?: number;
}) {
  const entries = Object.entries(categories).filter(([, n]) => n > 0);
  const durSec = duration && duration > 0 ? Math.round(duration / 1000) : 0;
  if (entries.length === 0) {
    return <span className="tele-phase">no tools</span>;
  }
  return (
    <span className="tele-cats">
      {entries.map(([cat, n]) => {
        const Icon = iconForCategory(cat) as React.ComponentType<{ size: number }>;
        return (
          <span key={cat} className="tele-cat">
            <Icon size={10} />
            <span className="tele-cat-n">{n}</span>
          </span>
        );
      })}
      {durSec > 0 && <span className="tele-dur">· {durSec}s</span>}
    </span>
  );
}

function phaseText(events: TraceEvent[], isActive: boolean): string | null {
  if (!isActive) return null;
  const phase = derivePhase(events);
  if (phase === "reading") return "reading…";
  if (phase === "thinking") return "thinking…";
  if (phase === "responding") return "responding…";
  return null;
}

function findLastIdx<T>(arr: T[], pred: (v: T) => boolean): number {
  for (let i = arr.length - 1; i >= 0; i--) {
    if (pred(arr[i])) return i;
  }
  return -1;
}

/** Extract tool name from data regardless of snake_case vs camelCase. */
function getToolName(data: Record<string, string>): string {
  return data.toolName ?? data.tool_name ?? "";
}

function getToolInput(data: Record<string, string>): string {
  return data.toolInput ?? data.tool_input ?? "";
}

/**
 * Merge tool_call + tool_result pairs and consecutive text events.
 * Filters out turn_end and reading events.
 */
function mergeEvents(events: TraceEvent[]): TraceEvent[] {
  const merged: TraceEvent[] = [];
  for (const e of events) {
    if (e.kind === "turn_end" || e.kind === "reading") continue;
    if (e.kind === "tool_result") {
      const idx = findLastIdx(
        merged,
        (m) => m.kind === "tool_call" || m.kind === "tool_done",
      );
      if (idx !== -1) {
        merged[idx] = {
          ...merged[idx],
          kind: "tool_done",
          timestampMs: e.timestampMs,
          data: {
            ...merged[idx].data,
            content: e.data.content ?? e.data.tool_result ?? "",
          },
        };
      }
      continue;
    }
    if (
      e.kind === "text" &&
      merged.length > 0 &&
      merged[merged.length - 1].kind === "text"
    ) {
      const last = merged[merged.length - 1];
      merged[merged.length - 1] = {
        ...last,
        data: { text: (last.data.text ?? "") + (e.data.text ?? "") },
      };
      continue;
    }
    merged.push(e);
  }
  return merged;
}

function mergeHistoryEvents(events: TraceEventRecord[]): TraceEventRecord[] {
  const merged: TraceEventRecord[] = [];
  for (const e of events) {
    const data = typeof e.data === "string" ? JSON.parse(e.data) : e.data;
    if (e.kind === "turn_end" || e.kind === "reading") continue;
    if (e.kind === "tool_result") {
      const idx = findLastIdx(
        merged,
        (m) => m.kind === "tool_call" || m.kind === "tool_done",
      );
      if (idx !== -1) {
        const md =
          typeof merged[idx].data === "string"
            ? JSON.parse(merged[idx].data as string)
            : merged[idx].data;
        merged[idx] = {
          ...merged[idx],
          kind: "tool_done",
          timestampMs: e.timestampMs,
          data: { ...md, content: data.content ?? "" },
        };
      }
      continue;
    }
    if (
      e.kind === "text" &&
      merged.length > 0 &&
      merged[merged.length - 1].kind === "text"
    ) {
      const last = merged[merged.length - 1];
      const lastData =
        typeof last.data === "string"
          ? JSON.parse(last.data as string)
          : last.data;
      merged[merged.length - 1] = {
        ...last,
        data: { text: (lastData.text ?? "") + (data.text ?? "") },
      };
      continue;
    }
    merged.push({ ...e, data });
  }
  return merged;
}

// ── Row helpers ──

/** Label shown when row is expanded (category name). */
function expandedLabel(kind: string, data: Record<string, string>): string {
  switch (kind) {
    case "thinking":
      return "thinking";
    case "tool_call":
      return getToolName(data) || "tool";
    case "tool_done":
      return `${getToolName(data) || "tool"} ✓`;
    case "text":
      return "response";
    case "error":
      return data.message ?? "error";
    default:
      return kind;
  }
}

/** Full content for expanded view. */
function fullContent(kind: string, data: Record<string, string>): string {
  switch (kind) {
    case "thinking":
      return data.text ?? "";
    case "tool_call":
      return getToolInput(data);
    case "tool_done":
      return data.content ?? "";
    case "text":
      return data.text ?? "";
    case "error":
      return data.message ?? "";
    default:
      return "";
  }
}

// ── Expandable row component ──

function ExpandableRow({
  kind,
  data,
}: {
  kind: string;
  data: Record<string, string>;
}) {
  const [expanded, setExpanded] = useState(false);
  const content = fullContent(kind, data);
  const canExpand = content.length > 50;
  const isTool = kind === "tool_call" || kind === "tool_done";

  return (
    <div className="tele-row">
      <div
        className={`tele-row-header${canExpand ? "" : " no-expand"}`}
        onClick={canExpand ? () => setExpanded(!expanded) : undefined}
      >
        <span className="tele-bullet">
          {canExpand ? (expanded ? "▾" : "▸") : "─"}
        </span>
        {expanded ? (
          <span className="tele-row-label">{expandedLabel(kind, data)}</span>
        ) : isTool ? (
          <>
            <span className="tele-tool-name">{expandedLabel(kind, data)}</span>
            {content && <span className="tele-pipe">|</span>}
            {content && (
              <span className="tele-tool-detail">{truncate(content, 50)}</span>
            )}
          </>
        ) : (
          <span className="tele-row-label">
            {truncate(content, 60) || kind}
          </span>
        )}
      </div>
      {expanded && content && <div className="tele-row-content">{content}</div>}
    </div>
  );
}

function TraceRow({
  kind,
  data,
}: {
  kind: string;
  data: Record<string, string>;
}) {
  return <ExpandableRow kind={kind} data={data} />;
}

// ── Main component ──

export function Telescope({
  agentName,
  runId,
  events,
  isActive,
  isError,
  onToggleExpand,
  isExpanded = true,
  traceSummary,
}: TelescopeProps) {
  const rowsRef = useRef<HTMLDivElement>(null);
  const isHistorical = !!traceSummary && events.length === 0;
  const [histExpanded, setHistExpanded] = useState(false);
  const [histEvents, setHistEvents] = useState<TraceEventRecord[] | null>(null);
  const [histLoading, setHistLoading] = useState(false);
  const isFlashing = useTraceStore(
    (s) => s.completionFlash[agentName] ?? false,
  );

  // Auto-scroll to bottom when new events arrive while active
  useEffect(() => {
    if (isExpanded && isActive && rowsRef.current) {
      rowsRef.current.scrollTop = rowsRef.current.scrollHeight;
    }
  }, [events.length, isExpanded, isActive]);

  const handleHistToggle = useCallback(() => {
    const next = !histExpanded;
    setHistExpanded(next);
    if (next && histEvents === null && !histLoading && runId) {
      setHistLoading(true);
      getTraceEvents(runId)
        .then((res) => setHistEvents(res.events))
        .catch(() => setHistEvents([]))
        .finally(() => setHistLoading(false));
    }
  }, [histExpanded, histEvents, histLoading, runId]);

  // ── Historical mode ──
  if (isHistorical) {
    return (
      <div
        className={`telescope${isError ? " error" : ""}${histExpanded ? " expanded" : ""}`}
      >
        <div className="tele-header" onClick={handleHistToggle}>
          <span className="tele-toggle">{histExpanded ? "▾" : "▸"}</span>
          <CategoryChips categories={traceSummary.categories} duration={traceSummary.duration} />
        </div>
        {histExpanded && (
          <div className="tele-rows" ref={rowsRef}>
            {histLoading && (
              <div className="tele-row">
                <div className="tele-row-header no-expand">
                  <span className="tele-bullet">─</span>
                  <span className="tele-row-label">loading…</span>
                </div>
              </div>
            )}
            {histEvents &&
              mergeHistoryEvents(histEvents).map((e) => {
                const data =
                  typeof e.data === "string" ? JSON.parse(e.data) : e.data;
                return <TraceRow key={e.seq} kind={e.kind} data={data} />;
              })}
          </div>
        )}
      </div>
    );
  }

  // ── Live mode ──
  const phase = derivePhase(events);
  const merged = mergeEvents(events);
  const isPreContent =
    phase === "reading" || (phase === "thinking" && merged.length === 0);

  // Show typing dots when agent is active but has no displayable rows yet
  if (isActive && isPreContent) {
    return (
      <div className="telescope">
        <div className="tele-header">
          <span className="tele-toggle">▸</span>
          <span className="tele-phase">
            {phase === "thinking" ? "thinking" : "reading"}
          </span>
          <span className="tele-typing-dots">
            <span>.</span>
            <span>.</span>
            <span>.</span>
          </span>
        </div>
      </div>
    );
  }

  if (events.length === 0) return null;

  const wrapperClass = `telescope${isError ? " error" : ""}${isFlashing ? " completion-flash" : ""}${isExpanded ? " expanded" : ""}`;

  return (
    <div className={wrapperClass}>
      <div className="tele-header" onClick={onToggleExpand}>
        <span className="tele-toggle">{isExpanded ? "▾" : "▸"}</span>
        {phaseText(events, isActive)
          ? <span className="tele-phase">{phaseText(events, isActive)}</span>
          : <CategoryChips categories={deriveCategories(events)} />}
      </div>
      {isExpanded && (
        <div className="tele-rows" ref={rowsRef}>
          {merged.map((e) => (
            <TraceRow key={e.seq} kind={e.kind} data={e.data} />
          ))}
        </div>
      )}
    </div>
  );
}
