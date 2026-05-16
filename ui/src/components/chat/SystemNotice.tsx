import { useNavigate } from "react-router-dom";
import type { HistoryMessage } from "../../data";
import { useAgents, useChannels, useHumans } from "../../hooks/data";
import { agentTabPath, channelPath } from "../../lib/routes";

interface SystemNoticeProps {
  message: HistoryMessage;
  // Rendered when the structured payload can't be resolved (deleted actor,
  // store still loading, missing fields, etc). Caller wires the existing
  // plain-text divider.
  fallback: React.ReactNode;
}

interface NoticeActor {
  id: string;
  type: "human" | "agent" | "system";
}

interface NoticeTarget {
  id: string;
  type: string;
  label: string;
}

interface ActorNoticeShape {
  actor: NoticeActor;
  verb: string;
  target?: NoticeTarget;
}

// Server-authored structured notice (e.g. member_joined, task_claimed).
// Rendered as a centered, all-mono row with three structural slots:
// `[actor chip] [verb] [target chip?]`. Actor and target are clickable
// chips that route to the entity profile / channel / task; verb is muted.
//
// The component is structural — it never switches on `payload.kind`. New
// kinds work the day they ship even on stale clients. If the payload
// doesn't have the required `actor`/`verb` shape, or actor lookup fails,
// the renderer falls back to the caller-provided plain divider so the
// historical English content stays visible.
export function SystemNotice({ message, fallback }: SystemNoticeProps) {
  const notice = narrowActorNotice(message.payload);
  if (!notice) return <>{fallback}</>;

  const fullTime = formatFullTime(message.createdAt);

  return (
    <div
      className="system-notice"
      role="status"
      aria-live="polite"
      title={fullTime}
    >
      <ActorChip actor={notice.actor} />
      <span className="system-notice__verb">{notice.verb}</span>
      {notice.target && <TargetChip target={notice.target} />}
    </div>
  );
}

function narrowActorNotice(
  payload: HistoryMessage["payload"],
): ActorNoticeShape | null {
  if (!payload) return null;
  const actor = payload.actor;
  if (!isNoticeActor(actor)) return null;
  if (typeof payload.verb !== "string") return null;
  const target = isNoticeTarget(payload.target) ? payload.target : undefined;
  return { actor, verb: payload.verb, target };
}

function isNoticeActor(value: unknown): value is NoticeActor {
  if (!value || typeof value !== "object") return false;
  const o = value as Record<string, unknown>;
  return (
    typeof o.id === "string" &&
    (o.type === "human" || o.type === "agent" || o.type === "system")
  );
}

function isNoticeTarget(value: unknown): value is NoticeTarget {
  if (!value || typeof value !== "object") return false;
  const o = value as Record<string, unknown>;
  return (
    typeof o.id === "string" &&
    typeof o.type === "string" &&
    typeof o.label === "string"
  );
}

function ActorChip({ actor }: { actor: NoticeActor }) {
  const agents = useAgents();
  const humans = useHumans();
  const navigate = useNavigate();

  if (actor.type === "agent") {
    const agent = agents.find((a) => a.id === actor.id);
    if (!agent) return <ActorPlaceholder id={actor.id} />;
    const label = agent.display_name ?? agent.name;
    return (
      <button
        type="button"
        className="system-notice__chip"
        onClick={() => navigate(agentTabPath(agent.name, "profile"))}
        title={`View ${label} profile`}
      >
        {label}
      </button>
    );
  }

  if (actor.type === "human") {
    const human = humans.find((h) => h.id === actor.id);
    if (!human) return <ActorPlaceholder id={actor.id} />;
    // Humans don't have a profile route yet — render the live name as a
    // span so renames flow through, but no click affordance.
    return <span className="system-notice__chip system-notice__chip--inert">{human.name}</span>;
  }

  // SenderType=='system' isn't a meaningful clickable actor. Render the id
  // as a literal so the line stays grammatical.
  return <ActorPlaceholder id={actor.id} />;
}

// Last-resort actor fallback when store lookup fails. Renders the id verbatim
// rather than disappearing, so the line still parses ("<id> joined #planning").
// The parent SystemNotice could also fall back to the plain divider, but doing
// that requires a hook-context decision; this localized placeholder keeps the
// hook tree stable when only one specific actor goes missing.
function ActorPlaceholder({ id }: { id: string }) {
  return <span className="system-notice__chip system-notice__chip--inert">{id}</span>;
}

function TargetChip({ target }: { target: NoticeTarget }) {
  const { allChannels } = useChannels();
  const navigate = useNavigate();

  if (target.type === "channel") {
    const channel = allChannels.find((c) => c.id === target.id);
    if (!channel) {
      return <span className="system-notice__chip system-notice__chip--inert">{target.label}</span>;
    }
    return (
      <button
        type="button"
        className="system-notice__chip"
        onClick={() => navigate(channelPath(channel.name))}
        title={`Open ${target.label}`}
      >
        {target.label}
      </button>
    );
  }

  // Unknown target.type — render the chip as inert text. We don't drop it;
  // the user still sees the structured shape, the click is a no-op until
  // a router for this target type ships.
  return <span className="system-notice__chip system-notice__chip--inert">{target.label}</span>;
}

function formatFullTime(iso: string): string {
  try {
    const d = new Date(iso);
    return `${d.toLocaleDateString([], {
      month: "short",
      day: "numeric",
      year: "numeric",
    })} ${d.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    })}`;
  } catch {
    return iso;
  }
}
