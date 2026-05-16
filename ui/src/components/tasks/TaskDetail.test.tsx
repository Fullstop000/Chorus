import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskDetailView } from "./TaskDetail";
import type { TaskInfo } from "../../data";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("TaskDetailView", () => {
  const target = {
    parentChannelId: "11111111-1111-1111-1111-111111111111",
    parentSlug: "eng",
    taskNumber: 7,
  };

  const baseProps = {
    target,
    advanceError: null,
    advancing: false,
    onBack: () => {},
    onAdvance: () => {},
    canAdvance: false,
    advanceLabel: null as string | null,
  };

  it("renders breadcrumbs and loading state when task is null", () => {
    const html = renderToStaticMarkup(
      <TaskDetailView {...baseProps} task={null} error={null} />,
    );

    expect(html).toContain("eng");
    expect(html).toContain("task #7");
    expect(html).toContain('aria-label="back to channel"');
    expect(html).toContain("Loading");
  });

  it("renders title, status, claimer, and creator when task is loaded", () => {
    const task: TaskInfo = {
      taskNumber: 7,
      title: "wire up the bridge",
      status: "in_progress",
      claimedByName: "alice",
      createdByName: "bob",
      subChannelId: "22222222-2222-2222-2222-222222222222",
      subChannelName: "eng__task-7",
    };
    const html = renderToStaticMarkup(
      <TaskDetailView {...baseProps} task={task} error={null} />,
    );

    expect(html).toContain("wire up the bridge");
    // Status enum is formatted for display (underscore → space) so the badge
    // reads "in progress", matching the board column headers and advance
    // button vocabulary instead of leaking the raw API enum.
    expect(html).toContain("in progress");
    expect(html).not.toContain("in_progress");
    expect(html).toContain("claimed by alice");
    expect(html).toContain("created by bob");
  });

  it("falls back to 'unknown' when creator is missing", () => {
    const task: TaskInfo = {
      taskNumber: 3,
      title: "orphan task",
      status: "todo",
      subChannelId: null,
      subChannelName: null,
    };
    const html = renderToStaticMarkup(
      <TaskDetailView {...baseProps} task={task} error={null} />,
    );

    expect(html).toContain("created by unknown");
  });

  it("renders the error message when fetch failed", () => {
    const html = renderToStaticMarkup(
      <TaskDetailView {...baseProps} task={null} error="HTTP 404" />,
    );

    expect(html).toContain("Failed to load task: HTTP 404");
  });

  it("renders the back button with the provided aria-label", () => {
    const onBack = vi.fn();
    const html = renderToStaticMarkup(
      <TaskDetailView
        {...baseProps}
        task={null}
        error={null}
        onBack={onBack}
      />,
    );
    expect(html).toContain('aria-label="back to channel"');
    expect(onBack).not.toHaveBeenCalled();
  });

  it("renders the advance button when canAdvance and subChannelId is set", () => {
    const task: TaskInfo = {
      taskNumber: 7,
      title: "wire it",
      status: "todo",
      subChannelId: "22222222-2222-2222-2222-222222222222",
      subChannelName: "eng__task-7",
    };
    const html = renderToStaticMarkup(
      <TaskDetailView
        {...baseProps}
        task={task}
        error={null}
        canAdvance
        advanceLabel="Start"
      />,
    );
    expect(html).toContain("Start");
    expect(html).toContain('class="task-detail__advance"');
  });

  it("disables the advance button with a tooltip for legacy tasks without a sub-channel", () => {
    const task: TaskInfo = {
      taskNumber: 3,
      title: "legacy",
      status: "todo",
      subChannelId: null,
      subChannelName: null,
    };
    const html = renderToStaticMarkup(
      <TaskDetailView
        {...baseProps}
        task={task}
        error={null}
        canAdvance
        advanceLabel="Start"
      />,
    );
    // Rendered, not hidden — silent hide leaves the user with no signal.
    expect(html).toContain("task-detail__advance");
    expect(html).toContain("disabled");
    // Tooltip explains why the button won't respond.
    expect(html).toMatch(/before sub-channels existed/i);
  });

  it("renders the advance button disabled with an owner tooltip when a non-claimer views the task", () => {
    const task: TaskInfo = {
      taskNumber: 7,
      title: "claimed by someone else",
      status: "in_progress",
      claimedByName: "alice",
      subChannelId: "22222222-2222-2222-2222-222222222222",
      subChannelName: "eng__task-7",
    };
    const html = renderToStaticMarkup(
      <TaskDetailView
        {...baseProps}
        task={task}
        error={null}
        canAdvance={false}
        advanceLabel="Submit for review"
      />,
    );
    // Button stays visible so non-claimers can see the next step exists and
    // who owns it. Hiding silently leaves them with no signal about the
    // permission model (Codex review C4).
    expect(html).toContain("task-detail__advance");
    expect(html).toContain("disabled");
    expect(html).toContain("Only alice can advance this task.");
  });

  it("shows advanceError banner when present", () => {
    const task: TaskInfo = {
      taskNumber: 7,
      title: "t",
      status: "in_progress",
      claimedByName: "alice",
      subChannelId: "22222222-2222-2222-2222-222222222222",
      subChannelName: "eng__task-7",
    };
    const html = renderToStaticMarkup(
      <TaskDetailView
        {...baseProps}
        task={task}
        error={null}
        advanceError="HTTP 403 not the claimer"
        canAdvance
        advanceLabel="Submit for review"
      />,
    );
    expect(html).toContain("Failed to advance task: HTTP 403 not the claimer");
  });
});

describe("getTaskDetail fetcher", () => {
  it("GETs the task detail endpoint with encoded channel id", async () => {
    const payload: TaskInfo = {
      taskNumber: 5,
      title: "t",
      status: "todo",
      subChannelId: "sub-1",
      subChannelName: "eng__task-5",
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(payload), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { getTaskDetail } = await import("../../data");
    const out = await getTaskDetail("abc def", 5);

    expect(fetchMock).toHaveBeenCalledOnce();
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe("/api/conversations/abc%20def/tasks/5");
    expect(out.title).toBe("t");
    expect(out.subChannelName).toBe("eng__task-5");

    vi.unstubAllGlobals();
  });

  it("surfaces HTTP errors instead of silently returning null", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ error: "task not found" }), {
        status: 404,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { getTaskDetail } = await import("../../data");
    await expect(getTaskDetail("cid", 99)).rejects.toThrow();

    vi.unstubAllGlobals();
  });
});
