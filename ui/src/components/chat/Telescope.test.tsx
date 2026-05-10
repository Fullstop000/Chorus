import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Telescope } from "./Telescope";
import type { TraceFrame } from "../../transport/types";

function traceFrame(
  overrides: Partial<TraceFrame> & Pick<TraceFrame, "kind">,
): TraceFrame {
  return {
    eventType: "agent.trace",
    runId: "run-1",
    agentId: "scout-id",
    seq: 1,
    timestampMs: 1,
    data: {},
    ...overrides,
  };
}

describe("Telescope loading state", () => {
  it("renders shimmering text while an active run is still reading", () => {
    const html = renderToStaticMarkup(
      <Telescope
        agentName="Scout"
        events={[]}
        isActive={true}
        isError={false}
      />,
    );

    expect(html).toContain("tele-shimmer");
    expect(html).toContain('data-phase="reading"');
    expect(html).not.toContain("tele-typing-dots");
  });

  it("keeps shimmering text while an active run is working through tools", () => {
    const html = renderToStaticMarkup(
      <Telescope
        agentName="Scout"
        events={[
          traceFrame({
            kind: "tool_call",
            data: {
              toolName: "bash",
              toolInput: "ls",
            },
          }),
        ]}
        isActive={true}
        isError={false}
      />,
    );

    expect(html).toContain("tele-shimmer");
    expect(html).toContain('data-phase="doing"');
    expect(html).toContain("tele-cats");
  });
});
