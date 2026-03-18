#!/usr/bin/env node

import path from "path";
import os from "os";
import { createRequire } from "module";
import { execSync } from "child_process";
import { accessSync } from "fs";
import { fileURLToPath } from "url";
import { DaemonConnection } from "./connection.ts";
import { AgentProcessManager } from "./agentProcessManager.ts";
import { RUNTIMES } from "../shared/src/index.ts";

var require2 = createRequire(import.meta.url);
var DAEMON_VERSION = require2("../package.json").version;

function detectRuntimes() {
  const detected = [];
  const cmd = process.platform === "win32" ? "where" : "which";
  for (const rt of RUNTIMES) {
    try {
      execSync(`${cmd} ${rt.binary}`, { stdio: "pipe" });
      detected.push(rt.id);
    } catch {
    }
  }
  return detected;
}

var args = process.argv.slice(2);
var serverUrl = "";
var apiKey = "";
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--server-url" && args[i + 1]) serverUrl = args[++i];
  if (args[i] === "--api-key" && args[i + 1]) apiKey = args[++i];
}
if (!serverUrl || !apiKey) {
  console.error("Usage: slock-daemon --server-url <url> --api-key <key>");
  process.exit(1);
}

var __dirname = path.dirname(fileURLToPath(import.meta.url));
var chatBridgePath = path.resolve(__dirname, "chat-bridge.js");
try {
  accessSync(chatBridgePath);
} catch {
  chatBridgePath = path.resolve(__dirname, "chat-bridge.ts");
}

var connection;
var agentManager = new AgentProcessManager(chatBridgePath, (msg) => {
  connection.send(msg);
}, apiKey);

connection = new DaemonConnection({
  serverUrl,
  apiKey,
  onMessage: (msg) => {
    console.log(`[Daemon] Received: ${msg.type}`, msg.type === "ping" ? "" : JSON.stringify(msg).slice(0, 200));
    switch (msg.type) {
      case "agent:start":
        console.log(`[Daemon] Starting agent ${msg.agentId} (model: ${msg.config.model}, session: ${msg.config.sessionId || "new"}${msg.wakeMessage ? ", with wake message" : ""})`);
        agentManager.startAgent(msg.agentId, msg.config, msg.wakeMessage, msg.unreadSummary).catch((err) => {
          const reason = err instanceof Error ? err.message : String(err);
          console.error(`[Daemon] Failed to start agent ${msg.agentId}:`, reason);
          connection.send({ type: "agent:status", agentId: msg.agentId, status: "inactive" });
          connection.send({ type: "agent:activity", agentId: msg.agentId, activity: "offline", detail: `Start failed: ${reason}` });
        });
        break;
      case "agent:stop":
        console.log(`[Daemon] Stopping agent ${msg.agentId}`);
        agentManager.stopAgent(msg.agentId);
        break;
      case "agent:sleep":
        console.log(`[Daemon] Sleeping agent ${msg.agentId}`);
        agentManager.sleepAgent(msg.agentId);
        break;
      case "agent:reset-workspace":
        console.log(`[Daemon] Resetting workspace for agent ${msg.agentId}`);
        agentManager.resetWorkspace(msg.agentId);
        break;
      case "agent:deliver":
        console.log(`[Daemon] Delivering message to ${msg.agentId}: ${msg.message.content.slice(0, 80)}`);
        agentManager.deliverMessage(msg.agentId, msg.message);
        connection.send({ type: "agent:deliver:ack", agentId: msg.agentId, seq: msg.seq });
        break;
      case "agent:workspace:list":
        agentManager.getFileTree(msg.agentId, msg.dirPath).then((files) => {
          connection.send({ type: "agent:workspace:file_tree", agentId: msg.agentId, files, dirPath: msg.dirPath });
        });
        break;
      case "agent:workspace:read":
        agentManager.readFile(msg.agentId, msg.path).then(({ content, binary }) => {
          connection.send({
            type: "agent:workspace:file_content",
            agentId: msg.agentId,
            requestId: msg.requestId,
            content,
            binary
          });
        }).catch(() => {
          connection.send({
            type: "agent:workspace:file_content",
            agentId: msg.agentId,
            requestId: msg.requestId,
            content: null,
            binary: false
          });
        });
        break;
      case "machine:workspace:scan":
        console.log("[Daemon] Scanning all workspace directories");
        agentManager.scanAllWorkspaces().then((directories) => {
          connection.send({ type: "machine:workspace:scan_result", directories });
        });
        break;
      case "machine:workspace:delete":
        console.log(`[Daemon] Deleting workspace directory: ${msg.directoryName}`);
        agentManager.deleteWorkspaceDirectory(msg.directoryName).then((success) => {
          connection.send({ type: "machine:workspace:delete_result", directoryName: msg.directoryName, success });
        });
        break;
      case "ping":
        connection.send({ type: "pong" });
        break;
    }
  },
  onConnect: () => {
    const runtimes = detectRuntimes();
    console.log(`[Daemon] Detected runtimes: ${runtimes.join(", ") || "none"}`);
    connection.send({
      type: "ready",
      capabilities: ["agent:start", "agent:stop", "agent:deliver", "workspace:files"],
      runtimes,
      runningAgents: agentManager.getRunningAgentIds(),
      hostname: os.hostname(),
      os: `${os.platform()} ${os.arch()}`,
      daemonVersion: DAEMON_VERSION
    });
  },
  onDisconnect: () => {
    console.log("[Daemon] Lost connection \u2014 agents continue running locally");
  }
});

console.log("[Slock Daemon] Starting...");
connection.connect();

var shutdown = async () => {
  console.log("[Slock Daemon] Shutting down...");
  await agentManager.stopAll();
  connection.disconnect();
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
