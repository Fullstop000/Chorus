import { spawn, execSync } from "child_process";
import { existsSync } from "fs";
import path from "path";
import { buildBaseSystemPrompt } from "./systemPrompt.ts";

var CodexDriver = class {
  id = "codex";
  supportsStdinNotification = false;
  mcpToolPrefix = "mcp_chat_";
  spawn(ctx) {
    const gitDir = path.join(ctx.workingDirectory, ".git");
    if (!existsSync(gitDir)) {
      execSync("git init", { cwd: ctx.workingDirectory, stdio: "pipe" });
      execSync("git add -A && git commit --allow-empty -m 'init'", {
        cwd: ctx.workingDirectory,
        stdio: "pipe",
        env: { ...process.env, GIT_AUTHOR_NAME: "slock", GIT_AUTHOR_EMAIL: "slock@local", GIT_COMMITTER_NAME: "slock", GIT_COMMITTER_EMAIL: "slock@local" }
      });
    }
    const isTsSource = ctx.chatBridgePath.endsWith(".ts");
    const command = isTsSource ? "npx" : "node";
    const bridgeArgs = isTsSource ? ["tsx", ctx.chatBridgePath, "--agent-id", ctx.agentId, "--server-url", ctx.config.serverUrl, "--auth-token", ctx.config.authToken || ctx.daemonApiKey] : [ctx.chatBridgePath, "--agent-id", ctx.agentId, "--server-url", ctx.config.serverUrl, "--auth-token", ctx.config.authToken || ctx.daemonApiKey];
    const args = ["exec"];
    if (ctx.config.sessionId) {
      args.push("resume", ctx.config.sessionId);
    }
    args.push(
      "--dangerously-bypass-approvals-and-sandbox",
      "--json"
    );
    args.push(
      "-c",
      `mcp_servers.chat.command=${JSON.stringify(command)}`,
      "-c",
      `mcp_servers.chat.args=${JSON.stringify(bridgeArgs)}`,
      "-c",
      "mcp_servers.chat.startup_timeout_sec=30",
      "-c",
      "mcp_servers.chat.tool_timeout_sec=120",
      "-c",
      "mcp_servers.chat.enabled=true",
      "-c",
      "mcp_servers.chat.required=true"
    );
    if (ctx.config.model) {
      args.push("-m", ctx.config.model);
    }
    if (ctx.config.reasoningEffort) {
      args.push("-c", `model_reasoning_effort=${ctx.config.reasoningEffort}`);
    }
    args.push(ctx.prompt);
    const spawnEnv = { ...process.env, FORCE_COLOR: "0", NO_COLOR: "1", ...ctx.config.envVars || {} };
    const proc = spawn("codex", args, {
      cwd: ctx.workingDirectory,
      stdio: ["pipe", "pipe", "pipe"],
      env: spawnEnv,
      shell: process.platform === "win32"
    });
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
      case "thread.started":
        if (event.thread_id) {
          events.push({ kind: "session_init", sessionId: event.thread_id });
        }
        break;
      case "turn.started":
        events.push({ kind: "thinking", text: "" });
        break;
      case "item.started":
      case "item.updated":
      case "item.completed": {
        const item = event.item;
        if (!item) break;
        switch (item.type) {
          case "reasoning":
            if (item.text) {
              events.push({ kind: "thinking", text: item.text });
            }
            break;
          case "agent_message":
            if (item.text && event.type === "item.completed") {
              events.push({ kind: "text", text: item.text });
            }
            break;
          case "command_execution":
            if (event.type === "item.started") {
              events.push({ kind: "tool_call", name: "shell", input: { command: item.command } });
            }
            break;
          case "file_change":
            if (event.type === "item.started" && Array.isArray(item.changes)) {
              for (const change of item.changes) {
                events.push({ kind: "tool_call", name: "file_change", input: { path: change.path, kind: change.kind } });
              }
            }
            break;
          case "mcp_tool_call":
            if (event.type === "item.started") {
              const toolName = item.server && item.tool ? `${this.mcpToolPrefix.replace(/_$/, "")}_${item.server}_${item.tool}` : item.tool || "mcp_tool";
              const name = item.server === "chat" ? `${this.mcpToolPrefix}${item.tool}` : toolName;
              events.push({ kind: "tool_call", name, input: item.arguments });
            }
            break;
          case "collab_tool_call":
            if (event.type === "item.started") {
              events.push({ kind: "tool_call", name: "collab_tool_call", input: {} });
            }
            break;
          case "todo_list":
            if (event.type === "item.started" || event.type === "item.updated") {
              events.push({ kind: "thinking", text: item.title || "Planning\u2026" });
            }
            break;
          case "web_search":
            if (event.type === "item.started") {
              events.push({ kind: "tool_call", name: "web_search", input: { query: item.query } });
            }
            break;
          case "error":
            if (item.message) {
              events.push({ kind: "error", message: item.message });
            }
            break;
        }
        break;
      }
      case "turn.completed":
        events.push({ kind: "turn_end" });
        break;
      case "turn.failed":
        if (event.error?.message) {
          events.push({ kind: "error", message: event.error.message });
        }
        events.push({ kind: "turn_end" });
        break;
      case "error":
        events.push({ kind: "error", message: event.message || "Unknown error" });
        break;
    }
    return events;
  }
  encodeStdinMessage(_text, _sessionId) {
    return null;
  }
  buildSystemPrompt(config, _agentId) {
    return buildBaseSystemPrompt(config, {
      toolPrefix: "",
      extraCriticalRules: [
        "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.",
        "- ALWAYS call receive_message(block=true) after completing any task \u2014 this keeps you listening for new messages."
      ],
      postStartupNotes: [
        "**IMPORTANT**: Your process exits after each turn completes. You will be automatically restarted when new messages arrive. Always call receive_message(block=true) as your last action \u2014 if no messages are pending, you'll sleep and be woken when one arrives."
      ],
      includeStdinNotificationSection: false
    });
  }
  toolDisplayName(name) {
    if (name === `${this.mcpToolPrefix}upload_file`) return "Uploading file\u2026";
    if (name === `${this.mcpToolPrefix}view_file`) return "Viewing file\u2026";
    if (name === `${this.mcpToolPrefix}list_tasks`) return "Listing tasks\u2026";
    if (name === `${this.mcpToolPrefix}create_tasks`) return "Creating tasks\u2026";
    if (name === `${this.mcpToolPrefix}claim_tasks`) return "Claiming tasks\u2026";
    if (name === `${this.mcpToolPrefix}unclaim_task`) return "Unclaiming task\u2026";
    if (name === `${this.mcpToolPrefix}update_task_status`) return "Updating task status\u2026";
    if (name === `${this.mcpToolPrefix}list_server`) return "Listing server\u2026";
    if (name === `${this.mcpToolPrefix}read_history`) return "Reading history\u2026";
    if (name.startsWith(this.mcpToolPrefix)) return "";
    if (name === "shell" || name === "command_execution") return "Running command\u2026";
    if (name === "file_change") return "Editing file\u2026";
    if (name === "file_read") return "Reading file\u2026";
    if (name === "file_write") return "Writing file\u2026";
    if (name === "web_search") return "Searching web\u2026";
    if (name === "collab_tool_call") return "Collaborating\u2026";
    return `Using ${name.length > 20 ? name.slice(0, 20) + "\u2026" : name}\u2026`;
  }
  summarizeToolInput(name, input) {
    if (!input || typeof input !== "object") return "";
    try {
      if (name === "shell" || name === "command_execution") {
        const cmd = input.command || "";
        return cmd.length > 100 ? cmd.slice(0, 100) + "\u2026" : cmd;
      }
      if (name === "file_change") return input.path || "";
      if (name === "file_read") return input.path || input.file_path || "";
      if (name === "file_write") return input.path || input.file_path || "";
      if (name === "web_search") return input.query || "";
      if (name === `${this.mcpToolPrefix}send_message`) {
        return input.target || input.channel || (input.dm_to ? `DM:@${input.dm_to}` : "");
      }
      if (name === `${this.mcpToolPrefix}read_history`) return input.target || input.channel || "";
      if (name === `${this.mcpToolPrefix}list_tasks`) return input.channel || "";
      if (name === `${this.mcpToolPrefix}create_tasks`) return input.channel || "";
      if (name === `${this.mcpToolPrefix}claim_tasks`) {
        const nums = input.task_numbers;
        return input.channel ? `${input.channel} #t${Array.isArray(nums) ? nums.join(",#t") : nums}` : "";
      }
      if (name === `${this.mcpToolPrefix}unclaim_task`) {
        return input.channel ? `${input.channel} #t${input.task_number}` : "";
      }
      if (name === `${this.mcpToolPrefix}update_task_status`) {
        return input.channel ? `${input.channel} #t${input.task_number}` : "";
      }
      if (name === `${this.mcpToolPrefix}upload_file`) return input.file_path || "";
      return "";
    } catch {
      return "";
    }
  }
};

export { CodexDriver };
