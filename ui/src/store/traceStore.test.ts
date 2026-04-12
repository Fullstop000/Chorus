import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.resetModules();
  vi.unstubAllGlobals();
});

describe("traceStore", () => {
  it("loads when localStorage is unavailable", async () => {
    await expect(import("./traceStore")).resolves.toHaveProperty(
      "useTraceStore",
    );
  });
});
