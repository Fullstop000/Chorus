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

  it("loads when localStorage.getItem throws", async () => {
    vi.stubGlobal("localStorage", {
      getItem: vi.fn(() => {
        throw new Error("localStorage unavailable");
      }),
      setItem: vi.fn(),
      removeItem: vi.fn(),
      clear: vi.fn(),
      key: vi.fn(),
      length: 0,
    });

    await expect(import("./traceStore")).resolves.toHaveProperty(
      "useTraceStore",
    );
  });
});
