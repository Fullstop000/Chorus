import { describe, expect, it } from "vitest";
import { getBottomTransition, isNearBottom } from "./MessageList";

describe("isNearBottom", () => {
  it("returns true when the scroll position is within the bottom threshold", () => {
    expect(
      isNearBottom({
        scrollHeight: 500,
        scrollTop: 291,
        clientHeight: 200,
      })
    ).toBe(true);
  });

  it("returns false when the scroll position is above the bottom threshold", () => {
    expect(
      isNearBottom({
        scrollHeight: 500,
        scrollTop: 280,
        clientHeight: 200,
      })
    ).toBe(false);
  });
});

describe("getBottomTransition", () => {
  it("returns entered when scrolling into the bottom zone", () => {
    expect(
      getBottomTransition(false, {
        scrollHeight: 500,
        scrollTop: 291,
        clientHeight: 200,
      })
    ).toBe("entered");
  });

  it("returns left when scrolling away from the bottom zone", () => {
    expect(
      getBottomTransition(true, {
        scrollHeight: 500,
        scrollTop: 280,
        clientHeight: 200,
      })
    ).toBe("left");
  });

  it("returns none when the bottom state does not change", () => {
    expect(
      getBottomTransition(true, {
        scrollHeight: 500,
        scrollTop: 295,
        clientHeight: 200,
      })
    ).toBe("none");
  });
});
