import { mkdir, writeFile, access, readdir, stat, readFile, rm } from "fs/promises";
import path from "path";
import os from "os";
import { getDriver } from "./drivers/index.ts";

var DATA_DIR = path.join(os.homedir(), ".slock", "agents");

function toLocalTime(iso) {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

var MAX_TRAJECTORY_TEXT = 2e3;

var AgentProcessManager = class {
  agents = /* @__PURE__ */ new Map();
  agentsStarting = /* @__PURE__ */ new Set();
  // Prevent concurrent starts of same agent
  chatBridgePath;
  sendToServer;
  daemonApiKey;
  constructor(chatBridgePath, sendToServer, daemonApiKey) {
    this.chatBridgePath = chatBridgePath;
    this.sendToServer = sendToServer;
    this.daemonApiKey = daemonApiKey;
  }
  async startAgent(agentId, config, wakeMessage, unreadSummary) {
    if (this.agents.has(agentId) || this.agentsStarting.has(agentId)) return;
    this.agentsStarting.add(agentId);
    try {
      const driver = getDriver(config.runtime || "claude");
      const agentDataDir = path.join(DATA_DIR, agentId);
      await mkdir(agentDataDir, { recursive: true });
      const memoryMdPath = path.join(agentDataDir, "MEMORY.md");
      try {
        await access(memoryMdPath);
      } catch {
        const agentName = config.displayName || config.name;
        const initialMemoryMd = `# ${agentName}

## Role
${config.description || "No role defined yet."}

## Key Knowledge
- No notes yet.

## Active Context
- First startup.
`;
        await writeFile(memoryMdPath, initialMemoryMd);
      }
      await mkdir(path.join(agentDataDir, "notes"), { recursive: true });
      const isResume = !!config.sessionId;
      let prompt;
      if (isResume && wakeMessage) {
        const channelLabel = wakeMessage.channel_type === "dm" ? `DM:@${wakeMessage.channel_name}` : `#${wakeMessage.channel_name}`;
        const senderPrefix = wakeMessage.sender_type === "agent" ? "(agent) " : "";
        const time = wakeMessage.timestamp ? ` (${toLocalTime(wakeMessage.timestamp)})` : "";
        const formatted = `[${channelLabel}]${time} ${senderPrefix}@${wakeMessage.sender_name}: ${wakeMessage.content}`;
        prompt = `New message received:\n\n${formatted}`;
        if (unreadSummary && Object.keys(unreadSummary).length > 0) {
          const otherUnread = Object.entries(unreadSummary).filter(([key]) => key !== channelLabel);
          if (otherUnread.length > 0) {
            prompt += `\n\nYou also have unread messages in other channels:`;
            for (const [ch, count] of otherUnread) {
              prompt += `\n- ${ch}: ${count} unread`;
            }
            prompt += `\n\nUse read_history to catch up, or respond to the message above first.`;
          }
        }
        prompt += `\n\nRespond as appropriate \u2014 reply using send_message, or take action as needed. Then call receive_message(block=true) to keep listening.`;
        if (driver.supportsStdinNotification) {
          prompt += `\n\nNote: While you are busy, you may receive [System notification: ...] messages. Finish your current step, then call receive_message to check.`;
        }
      } else if (isResume && unreadSummary && Object.keys(unreadSummary).length > 0) {
        prompt = `You have unread messages from while you were offline:`;
        for (const [ch, count] of Object.entries(unreadSummary)) {
          prompt += `\n- ${ch}: ${count} unread`;
        }
        prompt += `\n\nUse read_history to catch up on important channels, then call receive_message(block=true) to listen for new messages.`;
        if (driver.supportsStdinNotification) {
          prompt += `\n\nNote: While you are busy, you may receive [System notification: ...] messages. Finish your current step, then call receive_message to check.`;
        }
      } else if (isResume) {
        prompt = `No new messages while you were away. Call ${driver.mcpToolPrefix}receive_message(block=true) to listen for new messages.`;
        if (driver.supportsStdinNotification) {
          prompt += `\n\nNote: While you are busy, you may receive [System notification: ...] messages about new messages. Finish your current step, then call receive_message to check.`;
        }
      } else {
        prompt = driver.buildSystemPrompt(config, agentId);
      }
      const { process: proc } = driver.spawn({
        agentId,
        config,
        prompt,
        workingDirectory: agentDataDir,
        chatBridgePath: this.chatBridgePath,
        daemonApiKey: this.daemonApiKey
      });
      const agentProcess = {
        process: proc,
        driver,
        inbox: [],
        pendingReceive: null,
        config,
        sessionId: config.sessionId || null,
        isInReceiveMessage: false,
        notificationTimer: null,
        pendingNotificationCount: 0
      };
      this.agents.set(agentId, agentProcess);
      this.agentsStarting.delete(agentId);
      let buffer = "";
      proc.stdout?.on("data", (chunk) => {
        buffer += chunk.toString();
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";
        for (const line of lines) {
          if (!line.trim()) continue;
          const events = driver.parseLine(line);
          for (const event of events) {
            this.handleParsedEvent(agentId, event, driver);
          }
        }
      });
      proc.stderr?.on("data", (chunk) => {
        const text = chunk.toString().trim();
        if (!text) return;
        if (/Reconnecting\.\.\.|Falling back from WebSockets/i.test(text)) return;
        console.error(`[Agent ${agentId} stderr]: ${text}`);
      });
      proc.on("exit", (code) => {
        console.log(`[Agent ${agentId}] Process exited with code ${code}`);
        if (this.agents.has(agentId)) {
          const ap = this.agents.get(agentId);
          if (ap.process !== proc) return;
          if (ap.pendingReceive) {
            clearTimeout(ap.pendingReceive.timer);
            ap.pendingReceive.resolve([]);
          }
          if (ap.notificationTimer) {
            clearTimeout(ap.notificationTimer);
          }
          this.agents.delete(agentId);
          if (code === 0) {
            this.sendToServer({ type: "agent:status", agentId, status: "sleeping" });
            this.sendToServer({ type: "agent:activity", agentId, activity: "sleeping", detail: "" });
          } else {
            const reason = code === null ? "killed by signal" : `exit code ${code}`;
            console.error(`[Agent ${agentId}] Process crashed (${reason}) \u2014 marking inactive`);
            this.sendToServer({ type: "agent:status", agentId, status: "inactive" });
            this.sendToServer({ type: "agent:activity", agentId, activity: "offline", detail: `Crashed (${reason})` });
          }
        }
      });
      this.sendToServer({ type: "agent:status", agentId, status: "active" });
      this.sendToServer({ type: "agent:activity", agentId, activity: "working", detail: "Starting\u2026" });
    } catch (err) {
      this.agentsStarting.delete(agentId);
      throw err;
    }
  }
  async stopAgent(agentId) {
    const ap = this.agents.get(agentId);
    if (!ap) return;
    if (ap.pendingReceive) {
      clearTimeout(ap.pendingReceive.timer);
      ap.pendingReceive.resolve([]);
    }
    if (ap.notificationTimer) {
      clearTimeout(ap.notificationTimer);
    }
    this.agents.delete(agentId);
    ap.process.kill("SIGTERM");
    this.sendToServer({ type: "agent:status", agentId, status: "inactive" });
    this.sendToServer({ type: "agent:activity", agentId, activity: "offline", detail: "" });
  }
  /** Hibernate: kill process but keep status as "sleeping" (auto-wakes on next message via --resume) */
  sleepAgent(agentId) {
    const ap = this.agents.get(agentId);
    if (!ap) return;
    console.log(`[Agent ${agentId}] Hibernating (sleeping)`);
    if (ap.pendingReceive) {
      clearTimeout(ap.pendingReceive.timer);
      ap.pendingReceive.resolve([]);
    }
    if (ap.notificationTimer) {
      clearTimeout(ap.notificationTimer);
    }
    this.agents.delete(agentId);
    ap.process.kill("SIGTERM");
  }
  deliverMessage(agentId, message) {
    const ap = this.agents.get(agentId);
    if (!ap) return;
    if (ap.pendingReceive) {
      clearTimeout(ap.pendingReceive.timer);
      ap.pendingReceive.resolve([message]);
      ap.pendingReceive = null;
    } else {
      ap.inbox.push(message);
    }
    if (!ap.driver.supportsStdinNotification) return;
    if (ap.isInReceiveMessage) return;
    if (!ap.sessionId) return;
    ap.pendingNotificationCount++;
    if (!ap.notificationTimer) {
      ap.notificationTimer = setTimeout(() => {
        this.sendStdinNotification(agentId);
      }, 3e3);
    }
  }
  async resetWorkspace(agentId) {
    const agentDataDir = path.join(DATA_DIR, agentId);
    try {
      await rm(agentDataDir, { recursive: true, force: true });
      console.log(`[Agent ${agentId}] Workspace deleted: ${agentDataDir}`);
    } catch (err) {
      console.error(`[Agent ${agentId}] Failed to delete workspace:`, err);
    }
  }
  async stopAll() {
    const ids = [...this.agents.keys()];
    await Promise.all(ids.map((id) => this.stopAgent(id)));
  }
  getRunningAgentIds() {
    return [...this.agents.keys()];
  }
  // Machine-level workspace scanning
  async scanAllWorkspaces() {
    const results = [];
    let entries;
    try {
      entries = await readdir(DATA_DIR, { withFileTypes: true });
    } catch {
      return [];
    }
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const dirPath = path.join(DATA_DIR, entry.name);
      try {
        const dirContents = await readdir(dirPath, { withFileTypes: true });
        let totalSize = 0;
        let latestMtime = /* @__PURE__ */ new Date(0);
        let fileCount = 0;
        for (const item of dirContents) {
          const itemPath = path.join(dirPath, item.name);
          try {
            const info = await stat(itemPath);
            if (item.isFile()) {
              totalSize += info.size;
              fileCount++;
            }
            if (info.mtime > latestMtime) {
              latestMtime = info.mtime;
            }
          } catch {
            continue;
          }
        }
        results.push({
          directoryName: entry.name,
          totalSizeBytes: totalSize,
          lastModified: latestMtime.toISOString(),
          fileCount
        });
      } catch {
        continue;
      }
    }
    return results;
  }
  async deleteWorkspaceDirectory(directoryName) {
    if (directoryName.includes("/") || directoryName.includes("..") || directoryName.includes("\\")) {
      return false;
    }
    const targetDir = path.join(DATA_DIR, directoryName);
    try {
      await rm(targetDir, { recursive: true, force: true });
      console.log(`[Workspace] Deleted directory: ${targetDir}`);
      return true;
    } catch (err) {
      console.error(`[Workspace] Failed to delete directory ${targetDir}:`, err);
      return false;
    }
  }
  // Workspace file browsing
  async getFileTree(agentId, dirPath) {
    const agentDir = path.join(DATA_DIR, agentId);
    try {
      await stat(agentDir);
    } catch {
      return [];
    }
    let targetDir = agentDir;
    if (dirPath) {
      const resolved = path.resolve(agentDir, dirPath);
      if (!resolved.startsWith(agentDir + path.sep) && resolved !== agentDir) {
        return [];
      }
      targetDir = resolved;
    }
    return this.listDirectoryChildren(targetDir, agentDir);
  }
  async readFile(agentId, filePath) {
    const agentDir = path.join(DATA_DIR, agentId);
    const resolved = path.resolve(agentDir, filePath);
    if (!resolved.startsWith(agentDir + path.sep) && resolved !== agentDir) {
      throw new Error("Access denied");
    }
    const info = await stat(resolved);
    if (info.isDirectory()) throw new Error("Cannot read a directory");
    const TEXT_EXTENSIONS = /* @__PURE__ */ new Set([
      ".md",
      ".txt",
      ".json",
      ".js",
      ".ts",
      ".jsx",
      ".tsx",
      ".yaml",
      ".yml",
      ".toml",
      ".log",
      ".csv",
      ".xml",
      ".html",
      ".css",
      ".sh",
      ".py"
    ]);
    const ext = path.extname(resolved).toLowerCase();
    if (!TEXT_EXTENSIONS.has(ext) && ext !== "") {
      return { content: null, binary: true };
    }
    if (info.size > 1048576) throw new Error("File too large");
    const content = await readFile(resolved, "utf-8");
    return { content, binary: false };
  }
  // Private methods
  /** Handle a single ParsedEvent from any runtime driver */
  handleParsedEvent(agentId, event, driver) {
    const trajectory = [];
    let activity = "";
    let detail = "";
    const ap = this.agents.get(agentId);
    switch (event.kind) {
      case "session_init":
        if (ap) ap.sessionId = event.sessionId;
        this.sendToServer({ type: "agent:session", agentId, sessionId: event.sessionId });
        break;
      case "thinking": {
        const text = event.text.length > MAX_TRAJECTORY_TEXT ? event.text.slice(0, MAX_TRAJECTORY_TEXT) + "\u2026" : event.text;
        trajectory.push({ kind: "thinking", text });
        activity = "thinking";
        if (ap) ap.isInReceiveMessage = false;
        break;
      }
      case "text": {
        const text = event.text.length > MAX_TRAJECTORY_TEXT ? event.text.slice(0, MAX_TRAJECTORY_TEXT) + "\u2026" : event.text;
        trajectory.push({ kind: "text", text });
        activity = "thinking";
        if (ap) ap.isInReceiveMessage = false;
        break;
      }
      case "tool_call": {
        const toolName = event.name;
        const inputSummary = driver.summarizeToolInput(toolName, event.input);
        trajectory.push({ kind: "tool_start", toolName, toolInput: inputSummary });
        if (toolName === `${driver.mcpToolPrefix}receive_message`) {
          const isBlocking = event.input?.block !== false;
          if (isBlocking) {
            activity = "online";
          }
          if (ap) {
            ap.isInReceiveMessage = true;
            ap.pendingNotificationCount = 0;
            if (ap.notificationTimer) {
              clearTimeout(ap.notificationTimer);
              ap.notificationTimer = null;
            }
          }
        } else if (toolName === `${driver.mcpToolPrefix}send_message`) {
          activity = "working";
          detail = "Sending message\u2026";
          if (ap) ap.isInReceiveMessage = false;
        } else {
          activity = "working";
          detail = driver.toolDisplayName(toolName);
          if (ap) ap.isInReceiveMessage = false;
        }
        break;
      }
      case "turn_end":
        activity = "online";
        if (ap) {
          ap.isInReceiveMessage = false;
          if (event.sessionId) ap.sessionId = event.sessionId;
        }
        if (event.sessionId) {
          this.sendToServer({ type: "agent:session", agentId, sessionId: event.sessionId });
        }
        break;
      case "error":
        trajectory.push({ kind: "text", text: `Error: ${event.message}` });
        break;
    }
    if (activity) {
      this.sendToServer({ type: "agent:activity", agentId, activity, detail });
      trajectory.push({ kind: "status", activity, detail });
    }
    if (trajectory.length > 0) {
      this.sendToServer({ type: "agent:trajectory", agentId, entries: trajectory });
    }
  }
  /** Send a batched notification to the agent via stdin about pending messages */
  sendStdinNotification(agentId) {
    const ap = this.agents.get(agentId);
    if (!ap) return;
    const count = ap.pendingNotificationCount;
    ap.pendingNotificationCount = 0;
    ap.notificationTimer = null;
    if (count === 0) return;
    if (ap.isInReceiveMessage) return;
    if (!ap.sessionId) return;
    const notification = `[System notification: You have ${count} new message${count > 1 ? "s" : ""} waiting. Call receive_message to read ${count > 1 ? "them" : "it"} when you're ready.]`;
    console.log(`[Agent ${agentId}] Sending stdin notification: ${count} message(s)`);
    const encoded = ap.driver.encodeStdinMessage(notification, ap.sessionId);
    if (encoded) {
      ap.process.stdin?.write(encoded + "\n");
    }
  }
  /** List ONE level of a directory — directories returned without children (lazy-loaded on demand) */
  async listDirectoryChildren(dir, rootDir) {
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      return [];
    }
    entries.sort((a, b) => {
      if (a.isDirectory() && !b.isDirectory()) return -1;
      if (!a.isDirectory() && b.isDirectory()) return 1;
      return a.name.localeCompare(b.name);
    });
    const nodes = [];
    for (const entry of entries) {
      if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
      const fullPath = path.join(dir, entry.name);
      const relativePath = path.relative(rootDir, fullPath);
      let info;
      try {
        info = await stat(fullPath);
      } catch {
        continue;
      }
      if (entry.isDirectory()) {
        nodes.push({ name: entry.name, path: relativePath, isDirectory: true, size: 0, modifiedAt: info.mtime.toISOString() });
      } else {
        nodes.push({ name: entry.name, path: relativePath, isDirectory: false, size: info.size, modifiedAt: info.mtime.toISOString() });
      }
    }
    return nodes;
  }
};

export { AgentProcessManager };
