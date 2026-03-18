import { spawn } from "child_process";
import { writeFileSync } from "fs";
import path from "path";
import { buildBaseSystemPrompt } from "./systemPrompt.ts";

var ClaudeDriver = class {
  id = "claude";
  supportsStdinNotification = true;
  mcpToolPrefix = "mcp__chat__";
  spawn(ctx) {
    const mcpArgs = [
      ctx.chatBridgePath,
      "--agent-id",
      ctx.agentId,
      "--server-url",
      ctx.config.serverUrl,
      "--auth-token",
      ctx.config.authToken || ctx.daemonApiKey
    ];
    const isTsSource = ctx.chatBridgePath.endsWith(".ts");
    const mcpConfig = JSON.stringify({
      mcpServers: {
        chat: {
          command: isTsSource ? "npx" : "node",
          args: isTsSource ? ["tsx", ...mcpArgs] : mcpArgs
        }
      }
    });
    const mcpConfigPath = path.join(ctx.workingDirectory, ".slock-claude-mcp.json");
    writeFileSync(mcpConfigPath, mcpConfig, "utf8");
    const args = [
      "--allow-dangerously-skip-permissions",
      "--dangerously-skip-permissions",
      "--verbose",
      "--output-format",
      "stream-json",
      "--input-format",
      "stream-json",
      "--mcp-config",
      mcpConfigPath,
      "--model",
      ctx.config.model || "sonnet"
    ];
    if (ctx.config.sessionId) {
      args.push("--resume", ctx.config.sessionId);
    }
    const spawnEnv = { ...process.env, FORCE_COLOR: "0", ...ctx.config.envVars || {} };
    delete spawnEnv.CLAUDECODE;
    const proc = spawn("claude", args, {
      cwd: ctx.workingDirectory,
      stdio: ["pipe", "pipe", "pipe"],
      env: spawnEnv,
      shell: process.platform === "win32"
    });
    const stdinMsg = JSON.stringify({
      type: "user",
      message: {
        role: "user",
        content: [{ type: "text", text: ctx.prompt }]
      },
      ...ctx.config.sessionId ? { session_id: ctx.config.sessionId } : {}
    });
    proc.stdin?.write(stdinMsg + "\n");
    return { process: proc };
  }
  parseLine(line) {
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      return [];
    }
    const events = [];
    switch (event.type) {
      case "system":
        if (event.subtype === "init" && event.session_id) {
          events.push({ kind: "session_init", sessionId: event.session_id });
        }
        break;
      case "assistant": {
        const content = event.message?.content;
        if (Array.isArray(content)) {
          for (const block of content) {
            if (block.type === "thinking" && block.thinking) {
              events.push({ kind: "thinking", text: block.thinking });
            } else if (block.type === "text" && block.text) {
              events.push({ kind: "text", text: block.text });
            } else if (block.type === "tool_use") {
              events.push({ kind: "tool_call", name: block.name || "unknown_tool", input: block.input });
            }
          }
        }
        break;
      }
      case "result": {
        events.push({ kind: "turn_end", sessionId: event.session_id });
        break;
      }
    }
    return events;
  }
  encodeStdinMessage(text, sessionId) {
    return JSON.stringify({
      type: "user",
      message: {
        role: "user",
        content: [{ type: "text", text }]
      },
      ...sessionId ? { session_id: sessionId } : {}
    });
  }
  buildSystemPrompt(config, _agentId) {
    return buildBaseSystemPrompt(config, {
      toolPrefix: "mcp__chat__",
      extraCriticalRules: [
        "- Do NOT use bash/curl/sqlite to send or receive messages. The MCP tools handle everything."
      ],
      postStartupNotes: [],
      includeStdinNotificationSection: true
    });
  }
  toolDisplayName(name) {
    if (name === "mcp__chat__upload_file") return "Uploading file\u2026";
    if (name === "mcp__chat__view_file") return "Viewing file\u2026";
    if (name === "mcp__chat__list_tasks") return "Listing tasks\u2026";
    if (name === "mcp__chat__create_tasks") return "Creating tasks\u2026";
    if (name === "mcp__chat__claim_tasks") return "Claiming tasks\u2026";
    if (name === "mcp__chat__unclaim_task") return "Unclaiming task\u2026";
    if (name === "mcp__chat__update_task_status") return "Updating task status\u2026";
    if (name === "mcp__chat__list_server") return "Listing server\u2026";
    if (name === "mcp__chat__read_history") return "Reading history\u2026";
    if (name.startsWith("mcp__chat__")) return "";
    if (name === "Read" || name === "read_file") return "Reading file\u2026";
    if (name === "Write" || name === "write_file") return "Writing file\u2026";
    if (name === "Edit" || name === "edit_file") return "Editing file\u2026";
    if (name === "Bash" || name === "bash") return "Running command\u2026";
    if (name === "Glob" || name === "glob") return "Searching files\u2026";
    if (name === "Grep" || name === "grep") return "Searching code\u2026";
    if (name === "WebFetch" || name === "web_fetch") return "Fetching web\u2026";
    if (name === "WebSearch" || name === "web_search") return "Searching web\u2026";
    if (name === "TodoWrite") return "Updating tasks\u2026";
    return `Using ${name.length > 20 ? name.slice(0, 20) + "\u2026" : name}\u2026`;
  }
  summarizeToolInput(name, input) {
    if (!input || typeof input !== "object") return "";
    try {
      if (name === "Read" || name === "read_file") return input.file_path || input.path || "";
      if (name === "Write" || name === "write_file") return input.file_path || input.path || "";
      if (name === "Edit" || name === "edit_file") return input.file_path || input.path || "";
      if (name === "Bash" || name === "bash") {
        const cmd = input.command || "";
        return cmd.length > 100 ? cmd.slice(0, 100) + "\u2026" : cmd;
      }
      if (name === "Glob" || name === "glob") return input.pattern || "";
      if (name === "Grep" || name === "grep") return input.pattern || "";
      if (name === "WebFetch" || name === "web_fetch") return input.url || "";
      if (name === "WebSearch" || name === "web_search") return input.query || "";
      if (name === "mcp__chat__send_message") {
        return input.target || input.channel || (input.dm_to ? `DM:@${input.dm_to}` : "");
      }
      if (name === "mcp__chat__read_history") return input.target || input.channel || "";
      if (name === "mcp__chat__list_tasks") return input.channel || "";
      if (name === "mcp__chat__create_tasks") return input.channel || "";
      if (name === "mcp__chat__claim_tasks") {
        const nums = input.task_numbers;
        return input.channel ? `${input.channel} #t${Array.isArray(nums) ? nums.join(",#t") : nums}` : "";
      }
      if (name === "mcp__chat__unclaim_task") {
        return input.channel ? `${input.channel} #t${input.task_number}` : "";
      }
      if (name === "mcp__chat__update_task_status") {
        return input.channel ? `${input.channel} #t${input.task_number}` : "";
      }
      if (name === "mcp__chat__upload_file") return input.file_path || "";
      return "";
    } catch {
      return "";
    }
  }
};

export { ClaudeDriver };
