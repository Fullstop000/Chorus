import { useState, useEffect, useRef } from "react";
import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import {
  BrainCircuit,
  MessageSquare,
  FileText,
  FilePen,
  FileOutput,
  Terminal,
  Search,
  FolderSearch,
  Globe,
  Inbox,
  History,
  ClipboardList,
  CheckSquare,
  Upload,
  Server,
  Zap,
  Circle,
  ChevronDown,
  ChevronUp,
  AlertCircle,
} from "lucide-react";
import { useTraceStore } from "../../../store/traceStore";
import { getTraceEvents } from "../../../data/chat";
import { getAgentRuns } from "../../../data/agents";
import { iconForCategory, labelForCategory } from "../../../lib/toolCategories";
import type { TraceEventRecord } from "../../../data/chat";
import type { AgentRunInfo } from "../../../data/agents";
import type { TraceFrame } from "../../../transport/types";
import "./TelescopeActivity.css";

interface Props {
  agentId: string;
  agentName: string;
}

// ── Tool icon + label lookup ──

type ToolMeta = { icon: ReactNode; label: string };

function toolMeta(rawName: string): ToolMeta {
  const name = rawName ?? "";

  if (name.startsWith("mcp__chat__") || name.startsWith("chat__")) {
    const op = name.replace(/^(mcp__)?chat__/, "");
    const map: Record<string, ToolMeta> = {
      send_message: {
        icon: <MessageSquare size={13} />,
        label: "Send message",
      },
      receive_message: { icon: <Inbox size={13} />, label: "Receive message" },
      read_history: { icon: <History size={13} />, label: "Read history" },
      get_history: { icon: <History size={13} />, label: "Read history" },
      list_server: { icon: <Server size={13} />, label: "List server" },
      get_server_info: { icon: <Server size={13} />, label: "Server info" },
      list_tasks: { icon: <ClipboardList size={13} />, label: "List tasks" },
      create_tasks: {
        icon: <ClipboardList size={13} />,
        label: "Create tasks",
      },
      claim_tasks: { icon: <ClipboardList size={13} />, label: "Claim tasks" },
      unclaim_task: {
        icon: <ClipboardList size={13} />,
        label: "Unclaim task",
      },
      update_task_status: {
        icon: <CheckSquare size={13} />,
        label: "Update task",
      },
      upload_file: { icon: <Upload size={13} />, label: "Upload file" },
      view_file: { icon: <FileText size={13} />, label: "View file" },
      resolve_channel: { icon: <Server size={13} />, label: "Resolve channel" },
    };
    return map[op] ?? { icon: <Zap size={13} />, label: op };
  }

  const map: Record<string, ToolMeta> = {
    Read: { icon: <FileText size={13} />, label: "Read file" },
    read_file: { icon: <FileText size={13} />, label: "Read file" },
    Write: { icon: <FileOutput size={13} />, label: "Write file" },
    write_file: { icon: <FileOutput size={13} />, label: "Write file" },
    Edit: { icon: <FilePen size={13} />, label: "Edit file" },
    edit_file: { icon: <FilePen size={13} />, label: "Edit file" },
    Bash: { icon: <Terminal size={13} />, label: "Run command" },
    bash: { icon: <Terminal size={13} />, label: "Run command" },
    Grep: { icon: <Search size={13} />, label: "Search code" },
    grep: { icon: <Search size={13} />, label: "Search code" },
    Glob: { icon: <FolderSearch size={13} />, label: "Find files" },
    glob: { icon: <FolderSearch size={13} />, label: "Find files" },
    WebFetch: { icon: <Globe size={13} />, label: "Fetch URL" },
    web_fetch: { icon: <Globe size={13} />, label: "Fetch URL" },
    WebSearch: { icon: <Globe size={13} />, label: "Web search" },
    web_search: { icon: <Globe size={13} />, label: "Web search" },
    TodoWrite: { icon: <CheckSquare size={13} />, label: "Update todos" },
    Task: { icon: <Zap size={13} />, label: "Spawn agent" },
  };

  return (
    map[name] ?? { icon: <Zap size={13} />, label: name.replace(/_/g, " ") }
  );
}

// ── Helpers ──

// Backend sends snake_case (tool_name), WebSocket sends camelCase (toolName)
function getToolName(data: Record<string, string>): string {
  const raw = data.toolName ?? data.tool_name ?? "";
  // Guard against serialized "undefined" / "null"
  return raw === "undefined" || raw === "null" ? "" : raw;
}

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 60_000)}m`;
}

function formatRunTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ── Expandable text ──

function ExpandableText({
  text,
  maxLines = 3,
  markdown = false,
}: {
  text: string;
  maxLines?: number;
  markdown?: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const lines = text.split("\n");
  const needsExpand = lines.length > maxLines || text.length > 300;
  const display = expanded
    ? text
    : lines.slice(0, maxLines).join("\n").slice(0, 300);

  return (
    <span className="activity-expandable">
      <span
        className={`activity-expandable-text${expanded ? " activity-expandable-full" : ""}`}
      >
        {markdown ? (
          <ReactMarkdown
            components={{
              p: ({ children }) => <span className="ae-md-p">{children}</span>,
              code: ({ children }) => (
                <code className="ae-md-code">{children}</code>
              ),
              strong: ({ children }) => <strong>{children}</strong>,
              em: ({ children }) => <em>{children}</em>,
            }}
          >
            {display}
          </ReactMarkdown>
        ) : (
          display
        )}
        {!expanded && needsExpand && "…"}
      </span>
      {needsExpand && (
        <button
          className="activity-expand-btn"
          onClick={() => setExpanded((x) => !x)}
          title={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
          {expanded ? "less" : "more"}
        </button>
      )}
    </span>
  );
}

// ── Row renderers ──

function TraceRow({
  kind,
  data,
  timestampMs,
}: {
  kind: string;
  data: Record<string, string>;
  timestampMs: number;
}) {
  switch (kind) {
    case "thinking":
      return (
        <div className="activity-item activity-item-thinking">
          <span className="activity-item-icon activity-icon-think">
            <BrainCircuit size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Thinking</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.text ?? ""} maxLines={2} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );

    case "tool": {
      const rawName = getToolName(data);
      const meta = toolMeta(rawName);
      const label = meta.label || rawName || "Tool";
      const isSendMsg =
        rawName === "mcp__chat__send_message" ||
        rawName === "chat__send_message" ||
        rawName === "send_message";

      // Structured view for send_message: target / content / message id
      if (isSendMsg) {
        const result = data.resultContent ?? "";
        // Extract target — "Message sent to dm:@bytedance."
        const targetMatch = result.match(/Message sent to ([^.]+)\./);
        const target = targetMatch ? targetMatch[1] : "";
        // Extract message id
        const idMatch = result.match(/Message ID:\s*([0-9a-f-]+)/i);
        const msgId = idMatch ? idMatch[1] : "";
        // Prefer full tool_input (populated by rawInput fix) over the bridge's
        // truncated "\nSent:" echo which is capped at 300 chars.
        const rawInput = data.toolInput ?? data.tool_input ?? "";
        const inputContent =
          target && rawInput.startsWith(target + ": ")
            ? rawInput.slice(target.length + 2)
            : rawInput;
        // Fallback: extract from result "\nSent: <content>" (bridge workaround)
        const sentMatch = result.match(/\nSent:\s*([\s\S]+)$/);
        const sentContent = sentMatch ? sentMatch[1] : "";
        const content = inputContent || sentContent;

        return (
          <div className="activity-item activity-item-tool">
            <span className="activity-item-icon activity-icon-tool">
              {meta.icon}
            </span>
            <div className="activity-item-main">
              <div className="activity-item-heading">
                <span className="activity-item-label">{label}</span>
                {target && (
                  <span className="activity-item-meta ta-send-target">
                    → {target}
                  </span>
                )}
              </div>
              {content && (
                <div className="activity-item-body">
                  <ExpandableText text={content} maxLines={3} markdown />
                </div>
              )}
              {msgId && (
                <div className="activity-item-body activity-item-muted ta-msg-id">
                  {msgId}
                </div>
              )}
            </div>
            <span className="activity-item-time">{fmtTime(timestampMs)}</span>
          </div>
        );
      }

      return (
        <div className="activity-item activity-item-tool">
          <span className="activity-item-icon activity-icon-tool">
            {meta.icon}
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">{label}</span>
            </div>
            {data.resultContent && (
              <div className="activity-item-body activity-item-muted">
                <ExpandableText
                  text={data.resultContent}
                  maxLines={3}
                  markdown
                />
              </div>
            )}
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );
    }

    // Fallback for unmerged tool_call (no result yet, e.g. live streaming)
    case "tool_call": {
      const rawName = getToolName(data);
      const meta = toolMeta(rawName);
      const label = meta.label || rawName || "Tool";
      return (
        <div className="activity-item activity-item-tool">
          <span className="activity-item-icon activity-icon-tool">
            {meta.icon}
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">{label}</span>
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );
    }

    case "text":
      return (
        <div className="activity-item activity-item-text-entry">
          <span className="activity-item-icon activity-icon-text">
            <MessageSquare size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Output</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.text ?? ""} maxLines={4} markdown />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );

    case "turn_end":
      return (
        <div className="activity-item activity-item-start">
          <span className="activity-item-icon activity-icon-start">
            <CheckSquare size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Completed</span>
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );

    case "error":
      return (
        <div
          className="activity-item"
          style={{ borderColor: "var(--color-destructive)" }}
        >
          <span
            className="activity-item-icon"
            style={{
              color: "var(--color-destructive)",
              background:
                "color-mix(in srgb, var(--color-destructive) 12%, transparent)",
            }}
          >
            <AlertCircle size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Error</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.message ?? ""} maxLines={3} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      );

    default:
      return null;
  }
}

// Coalesce frames: merge thinking/text runs, merge tool_call + tool_result pairs
function coalesceFrames<
  T extends { kind: string; data: Record<string, string> },
>(frames: T[]): T[] {
  const result: T[] = [];
  for (let i = 0; i < frames.length; i++) {
    const frame = frames[i];
    const last = result[result.length - 1];

    // Merge consecutive thinking or text
    if (
      last &&
      last.kind === frame.kind &&
      (frame.kind === "thinking" || frame.kind === "text")
    ) {
      result[result.length - 1] = {
        ...last,
        data: {
          ...last.data,
          text: (last.data.text ?? "") + (frame.data.text ?? ""),
        },
      };
      continue;
    }

    // Merge tool_call with its following tool_result
    if (frame.kind === "tool_call") {
      const next = frames[i + 1];
      if (next && next.kind === "tool_result") {
        const name = getToolName(frame.data) || getToolName(next.data);
        result.push({
          ...frame,
          kind: "tool" as T["kind"],
          data: {
            ...frame.data,
            toolName: name,
            resultContent: next.data.content ?? "",
          },
        } as T);
        i++; // skip the tool_result
        continue;
      }
    }

    // Drop standalone tool_result (already merged above)
    if (frame.kind === "tool_result") {
      continue;
    }

    result.push(frame);
  }
  return result;
}

// ── Category chips ──

function CategoryChips({ categories }: { categories: Record<string, number> }) {
  const entries = Object.entries(categories).filter(([, n]) => n > 0);
  if (entries.length === 0)
    return <span className="ta-run-tools">0 tools</span>;
  return (
    <span className="ta-run-tools ta-run-cats">
      {entries.map(([cat, n]) => {
        const Icon = iconForCategory(cat) as React.ComponentType<{
          size: number;
        }>;
        return (
          <span
            key={cat}
            className="ta-run-cat"
            data-tip={labelForCategory(cat)}
          >
            <Icon size={10} />
            <span>{n}</span>
          </span>
        );
      })}
    </span>
  );
}

// ── Runs list sidebar item ──

function RunItem({
  run,
  isSelected,
  isLive,
  onSelect,
}: {
  run: AgentRunInfo;
  isSelected: boolean;
  isLive: boolean;
  onSelect: () => void;
}) {
  const ts = run.traceSummary;
  const dur = ts.duration > 0 ? formatDuration(ts.duration) : "—";

  return (
    <button
      className={`ta-run-item${isSelected ? " ta-run-selected" : ""}${isLive ? " ta-run-live" : ""}`}
      onClick={onSelect}
    >
      <div className="ta-run-top">
        <span className="ta-run-time">{formatRunTime(run.createdAt)}</span>
        <span className={`ta-run-status ta-status-${ts.status}`}>
          {isLive ? (
            <>
              <Circle size={6} fill="currentColor" /> live
            </>
          ) : (
            ts.status
          )}
        </span>
      </div>
      <div className="ta-run-bottom">
        <CategoryChips categories={ts.categories} />
        <span className="ta-run-dur">{dur}</span>
      </div>
    </button>
  );
}

// ── Main component ──

export function TelescopeActivity({ agentId, agentName }: Props) {
  const trace = useTraceStore((s) => s.traces[agentName]);
  const listRef = useRef<HTMLDivElement>(null);

  // Runs list
  const [runs, setRuns] = useState<AgentRunInfo[]>([]);
  const [runsLoading, setRunsLoading] = useState(true);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

  // Trace events for selected run
  const [traceEvents, setTraceEvents] = useState<TraceEventRecord[] | null>(
    null,
  );
  const [traceLoading, setTraceLoading] = useState(false);

  // Fetch runs list
  useEffect(() => {
    let cancelled = false;
    setRunsLoading(true);
    getAgentRuns(agentId)
      .then((res) => {
        if (cancelled) return;
        setRuns(res.runs);
        setRunsLoading(false);
      })
      .catch(() => {
        if (!cancelled) setRunsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId]);

  // Auto-select: live run first, else most recent
  const liveRunId = trace?.isActive ? trace.runId : null;
  const effectiveSelectedId =
    selectedRunId ?? liveRunId ?? (runs.length > 0 ? runs[0].runId : null);

  // Fetch trace events when a historical run is selected
  useEffect(() => {
    if (!effectiveSelectedId) return;
    // If it's the live run, skip API fetch — we use WebSocket data
    if (liveRunId && effectiveSelectedId === liveRunId) {
      setTraceEvents(null);
      return;
    }
    let cancelled = false;
    setTraceLoading(true);
    getTraceEvents(effectiveSelectedId)
      .then((res) => {
        if (cancelled) return;
        setTraceEvents(res.events);
        setTraceLoading(false);
      })
      .catch(() => {
        if (!cancelled) {
          setTraceEvents(null);
          setTraceLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [effectiveSelectedId, liveRunId]);

  // Auto-scroll trace list
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [trace?.events.length, traceEvents?.length]);

  // Build trace rows
  const isShowingLive = liveRunId != null && effectiveSelectedId === liveRunId;
  const liveRows = isShowingLive && trace ? coalesceFrames(trace.events) : [];
  const histRows =
    !isShowingLive && traceEvents
      ? coalesceFrames(
          traceEvents.map((e) => ({
            ...e,
            data: typeof e.data === "string" ? JSON.parse(e.data) : e.data,
          })),
        )
      : [];
  const hasRows = liveRows.length > 0 || histRows.length > 0;

  // Status for current view
  const isActive = isShowingLive && (trace?.isActive ?? false);
  const isError = isShowingLive && (trace?.isError ?? false);
  const statusLabel = isError
    ? "Error"
    : isActive
      ? "Active"
      : effectiveSelectedId
        ? "Completed"
        : "Idle";
  const dotColor = isError
    ? "var(--color-destructive)"
    : isActive
      ? "var(--status-online)"
      : effectiveSelectedId
        ? "var(--status-sleeping)"
        : "var(--status-inactive)";

  return (
    <div className="ta-layout">
      {/* ── Runs sidebar ── */}
      <div className="ta-runs-sidebar">
        <div className="ta-runs-header">
          <span className="ta-runs-title">— runs</span>
        </div>
        <div className="ta-runs-list">
          {runsLoading && <div className="ta-runs-empty">Loading…</div>}
          {!runsLoading && runs.length === 0 && !liveRunId && (
            <div className="ta-runs-empty">No runs yet.</div>
          )}
          {liveRunId && (
            <RunItem
              key={liveRunId}
              run={{
                runId: liveRunId,
                messageId: "",
                createdAt: new Date().toISOString(),
                traceSummary: {
                  toolCalls:
                    trace?.events.filter((e) => e.kind === "tool_call")
                      .length ?? 0,
                  duration: 0,
                  status: "completed",
                  categories: {},
                },
              }}
              isSelected={effectiveSelectedId === liveRunId}
              isLive={true}
              onSelect={() => setSelectedRunId(liveRunId)}
            />
          )}
          {runs
            .filter((r) => r.runId !== liveRunId)
            .map((run) => (
              <RunItem
                key={run.runId}
                run={run}
                isSelected={effectiveSelectedId === run.runId}
                isLive={false}
                onSelect={() => setSelectedRunId(run.runId)}
              />
            ))}
        </div>
      </div>

      {/* ── Trace detail ── */}
      <div className="ta-detail">
        <div className="ta-detail-header">
          <span className="ta-detail-title">
            — {agentName.toLowerCase()} trace
          </span>
          <span className="ta-detail-status" style={{ color: dotColor }}>
            <Circle size={7} fill="currentColor" />
            <span>{statusLabel}</span>
          </span>
        </div>
        {!hasRows && !traceLoading && (
          <div className="ta-detail-empty">
            {effectiveSelectedId
              ? "No trace events."
              : "Select a run to view its trace."}
          </div>
        )}
        {traceLoading && !hasRows && (
          <div className="ta-detail-empty">Loading trace…</div>
        )}
        {hasRows && (
          <div className="activity-list" ref={listRef}>
            {liveRows.map((frame: TraceFrame) => (
              <TraceRow
                key={frame.seq}
                kind={frame.kind}
                data={frame.data}
                timestampMs={frame.timestampMs}
              />
            ))}
            {histRows.map((e) => (
              <TraceRow
                key={e.seq}
                kind={e.kind}
                data={e.data}
                timestampMs={e.timestampMs}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
