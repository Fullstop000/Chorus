import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Telescope } from "./Telescope";

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
    expect(html).toContain("reading…");
    expect(html).not.toContain("tele-typing-dots");
  });
});
