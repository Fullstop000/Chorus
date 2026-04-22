import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskDetailView } from "./TaskDetail";
import { useStore } from "../../store";
import type { TaskInfo } from "../../data";

afterEach(() => {
  useStore.getState().setCurrentTaskDetail(null);
  vi.restoreAllMocks();
});

describe("TaskDetailView", () => {
  const target = {
    parentChannelId: "11111111-1111-1111-1111-111111111111",
    parentSlug: "eng",
    taskNumber: 7,
  };

  it("renders breadcrumbs and loading state when task is null", () => {
    const html = renderToStaticMarkup(
      <TaskDetailView
        target={target}
        task={null}
        error={null}
        onBack={() => {}}
      />,
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
      <TaskDetailView
        target={target}
        task={task}
        error={null}
        onBack={() => {}}
      />,
    );

    expect(html).toContain("wire up the bridge");
    expect(html).toContain("in_progress");
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
      <TaskDetailView
        target={target}
        task={task}
        error={null}
        onBack={() => {}}
      />,
    );

    expect(html).toContain("created by unknown");
  });

  it("renders the error message when fetch failed", () => {
    const html = renderToStaticMarkup(
      <TaskDetailView
        target={target}
        task={null}
        error="HTTP 404"
        onBack={() => {}}
      />,
    );

    expect(html).toContain("Failed to load task: HTTP 404");
  });

  it("renders the back button with the provided aria-label", () => {
    const onBack = vi.fn();
    const html = renderToStaticMarkup(
      <TaskDetailView
        target={target}
        task={null}
        error={null}
        onBack={onBack}
      />,
    );
    expect(html).toContain('aria-label="back to channel"');
    expect(onBack).not.toHaveBeenCalled();
  });
});

describe("currentTaskDetail store behaviour", () => {
  it("setCurrentTaskDetail stores and clears the target", () => {
    useStore.getState().setCurrentTaskDetail({
      parentChannelId: "cid",
      parentSlug: "design",
      taskNumber: 3,
    });
    expect(useStore.getState().currentTaskDetail).toEqual({
      parentChannelId: "cid",
      parentSlug: "design",
      taskNumber: 3,
    });

    useStore.getState().setCurrentTaskDetail(null);
    expect(useStore.getState().currentTaskDetail).toBeNull();
  });

  it("switching to a different parent channel discards stale task detail", () => {
    useStore.getState().setCurrentTaskDetail({
      parentChannelId: "cid-a",
      parentSlug: "alpha",
      taskNumber: 1,
    });

    useStore.getState().setCurrentChannel({
      id: "cid-b",
      name: "beta",
      joined: true,
      channel_type: "team",
    });

    expect(useStore.getState().currentTaskDetail).toBeNull();
  });

  it("re-selecting the same parent channel preserves the open task detail", () => {
    useStore.getState().setCurrentTaskDetail({
      parentChannelId: "cid-keep",
      parentSlug: "keep",
      taskNumber: 9,
    });

    useStore.getState().setCurrentChannel({
      id: "cid-keep",
      name: "keep",
      joined: true,
      channel_type: "team",
    });

    expect(useStore.getState().currentTaskDetail).toEqual({
      parentChannelId: "cid-keep",
      parentSlug: "keep",
      taskNumber: 9,
    });
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
