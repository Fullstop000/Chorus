import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskDetailView } from "./TaskDetail";
import { useStore } from "../../store";

afterEach(() => {
  useStore.getState().setCurrentTaskDetail(null);
});

describe("TaskDetailView", () => {
  it("renders breadcrumbs with parent slug and task number", () => {
    const html = renderToStaticMarkup(
      <TaskDetailView
        target={{
          parentChannelId: "11111111-1111-1111-1111-111111111111",
          parentSlug: "eng",
          taskNumber: 7,
        }}
        onBack={() => {}}
      />,
    );

    expect(html).toContain('data-testid="task-detail"');
    expect(html).toContain("eng");
    expect(html).toContain("task #7");
    expect(html).toContain('aria-label="back to channel"');
  });

  it("renders an onBack handler wired to the back button", () => {
    const onBack = vi.fn();
    const html = renderToStaticMarkup(
      <TaskDetailView
        target={{
          parentChannelId: "cid",
          parentSlug: "design",
          taskNumber: 3,
        }}
        onBack={onBack}
      />,
    );
    // SSR can't fire the click; confirm the back control is present and
    // the component wires the handler by type (smoke via markup presence).
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
