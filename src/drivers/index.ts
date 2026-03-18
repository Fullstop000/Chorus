import { ClaudeDriver } from "./claude.ts";
import { CodexDriver } from "./codex.ts";

var drivers = {
  claude: new ClaudeDriver(),
  codex: new CodexDriver()
};

function getDriver(runtimeId) {
  const driver = drivers[runtimeId];
  if (!driver) {
    throw new Error(`Unknown runtime: ${runtimeId}. Available: ${Object.keys(drivers).join(", ")}`);
  }
  return driver;
}

export { drivers, getDriver };
