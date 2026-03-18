#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

function toLocalTime(iso) {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

var args = process.argv.slice(2);
var agentId = "";
var serverUrl = "http://localhost:3001";
var authToken = "";
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--agent-id" && args[i + 1]) agentId = args[++i];
  if (args[i] === "--server-url" && args[i + 1]) serverUrl = args[++i];
  if (args[i] === "--auth-token" && args[i + 1]) authToken = args[++i];
}
if (!agentId) {
  console.error("Missing --agent-id");
  process.exit(1);
}

var commonHeaders = { "Content-Type": "application/json" };
if (authToken) {
  commonHeaders["Authorization"] = `Bearer ${authToken}`;
}

function formatTarget(m) {
  if (m.channel_type === "thread" && m.parent_channel_name) {
    const shortId = m.channel_name.startsWith("thread-") ? m.channel_name.slice(7) : m.channel_name;
    if (m.parent_channel_type === "dm") {
      return `dm:@${m.parent_channel_name}:${shortId}`;
    }
    return `#${m.parent_channel_name}:${shortId}`;
  }
  if (m.channel_type === "dm") {
    return `dm:@${m.channel_name}`;
  }
  return `#${m.channel_name}`;
}

var server = new McpServer({
  name: "chat",
  version: "1.0.0"
});

server.tool(
  "send_message",
  "Send a message to a channel, DM, or thread. Use the target value from received messages to reply. Format: '#channel' for channels, 'dm:@peer' for DMs, '#channel:shortid' for threads in channels, 'dm:@peer:shortid' for threads in DMs. To start a NEW DM, use 'dm:@person-name'.",
  {
    target: z.string().describe(
      "Where to send. Reuse the identifier from received messages. Format: '#channel' for channels, 'dm:@name' for DMs, '#channel:id' for channel threads, 'dm:@name:id' for DM threads. Examples: '#general', 'dm:@richard', '#general:abcd1234', 'dm:@richard:abcd1234'."
    ),
    content: z.string().describe("The message content"),
    attachment_ids: z.array(z.string()).optional().describe("Optional attachment IDs from upload_file to include with the message")
  },
  async ({ target, content, attachment_ids }) => {
    try {
      const res = await fetch(`${serverUrl}/internal/agent/${agentId}/send`, {
        method: "POST",
        headers: commonHeaders,
        body: JSON.stringify({ target, content, attachmentIds: attachment_ids })
      });
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [
            { type: "text", text: `Error: ${data.error}` }
          ]
        };
      }
      const shortId = data.messageId ? data.messageId.slice(0, 8) : null;
      const replyHint = shortId ? ` (to reply in this message's thread, use target "${target.includes(":") ? target : target + ":" + shortId}")` : "";
      return {
        content: [
          {
            type: "text",
            text: `Message sent to ${target}. Message ID: ${data.messageId}${replyHint}`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "upload_file",
  "Upload an image file to attach to a message. Returns an attachment ID that you can pass to send_message's attachment_ids parameter. Supported formats: JPEG, PNG, GIF, WebP. Max size: 5MB.",
  {
    file_path: z.string().describe("Absolute path to the image file on your local filesystem"),
    channel: z.string().describe("The channel target where this file will be used (e.g. '#general', 'dm:@richard')")
  },
  async ({ file_path, channel }) => {
    try {
      const fs = await import("fs");
      const path = await import("path");
      if (!fs.existsSync(file_path)) {
        return {
          content: [{ type: "text", text: `Error: File not found: ${file_path}` }]
        };
      }
      const stat = fs.statSync(file_path);
      if (stat.size > 5 * 1024 * 1024) {
        return {
          content: [{ type: "text", text: `Error: File too large (${(stat.size / 1024 / 1024).toFixed(1)}MB). Max 5MB.` }]
        };
      }
      const listRes = await fetch(`${serverUrl}/internal/agent/${agentId}/resolve-channel`, {
        method: "POST",
        headers: commonHeaders,
        body: JSON.stringify({ target: channel })
      });
      let channelId;
      if (listRes.ok) {
        const listData = await listRes.json();
        channelId = listData.channelId;
      } else {
        return {
          content: [{ type: "text", text: `Error: Could not resolve channel: ${channel}` }]
        };
      }
      const fileBuffer = fs.readFileSync(file_path);
      const filename = path.basename(file_path);
      const ext = path.extname(file_path).toLowerCase();
      const mimeMap = {
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".png": "image/png",
        ".gif": "image/gif",
        ".webp": "image/webp"
      };
      const mimeType = mimeMap[ext] || "application/octet-stream";
      const blob = new Blob([fileBuffer], { type: mimeType });
      const formData = new FormData();
      formData.append("file", blob, filename);
      formData.append("channelId", channelId);
      const uploadHeaders = {};
      if (authToken) {
        uploadHeaders["Authorization"] = `Bearer ${authToken}`;
      }
      const res = await fetch(`${serverUrl}/internal/agent/${agentId}/upload`, {
        method: "POST",
        headers: uploadHeaders,
        body: formData
      });
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      return {
        content: [
          {
            type: "text",
            text: `File uploaded: ${data.filename} (${(data.sizeBytes / 1024).toFixed(1)}KB)\nAttachment ID: ${data.id}\n\nUse this ID in send_message's attachment_ids parameter to include it in a message.`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "view_file",
  "Download an attached image by its attachment ID and save it locally so you can view it. Returns the local file path. Use this when you see '[use view_file to see]' in a message with images.",
  {
    attachment_id: z.string().describe("The attachment UUID (from the 'id:...' shown in the message)")
  },
  async ({ attachment_id }) => {
    try {
      const fs = await import("fs");
      const path = await import("path");
      const os = await import("os");
      const cacheDir = path.join(os.homedir(), ".slock", "attachments");
      fs.mkdirSync(cacheDir, { recursive: true });
      const existing = fs.readdirSync(cacheDir).find((f) => f.startsWith(attachment_id));
      if (existing) {
        const cachedPath = path.join(cacheDir, existing);
        return {
          content: [{ type: "text", text: `File already cached at: ${cachedPath}\n\nUse your Read tool to view this image.` }]
        };
      }
      const downloadHeaders = {};
      if (authToken) {
        downloadHeaders["Authorization"] = `Bearer ${authToken}`;
      }
      const res = await fetch(`${serverUrl}/api/attachments/${attachment_id}`, {
        headers: downloadHeaders,
        redirect: "follow"
      });
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: Failed to download attachment (${res.status})` }]
        };
      }
      const contentType = res.headers.get("content-type") || "application/octet-stream";
      const extMap = {
        "image/jpeg": ".jpg",
        "image/png": ".png",
        "image/gif": ".gif",
        "image/webp": ".webp"
      };
      const ext = extMap[contentType] || ".bin";
      const filePath = path.join(cacheDir, `${attachment_id}${ext}`);
      const buffer = Buffer.from(await res.arrayBuffer());
      fs.writeFileSync(filePath, buffer);
      return {
        content: [{ type: "text", text: `Downloaded to: ${filePath}\n\nUse your Read tool to view this image.` }]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "receive_message",
  "Receive new messages. Use block=true to wait for new messages. Returns messages formatted as [#channel], [dm:@peer], or [thread:#channel:id] followed by the sender and content.",
  {
    block: z.boolean().default(true).describe("Whether to block (wait) for new messages"),
    timeout_ms: z.number().default(59e3).describe("How long to wait in ms when blocking (default 59s, just under MCP tool call timeout)")
  },
  async ({ block, timeout_ms }) => {
    try {
      const params = new URLSearchParams();
      if (block) params.set("block", "true");
      params.set("timeout", String(timeout_ms));
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/receive?${params}`,
        { method: "GET", headers: commonHeaders }
      );
      const data = await res.json();
      if (!data.messages || data.messages.length === 0) {
        return {
          content: [{ type: "text", text: "No new messages." }]
        };
      }
      const formatted = data.messages.map((m) => {
        const target = formatTarget(m);
        const msgId = m.message_id ? m.message_id.slice(0, 8) : "-";
        const time = m.timestamp ? toLocalTime(m.timestamp) : "-";
        const senderType = m.sender_type === "agent" ? " type=agent" : "";
        const attachSuffix = m.attachments?.length ? ` [${m.attachments.length} image${m.attachments.length > 1 ? "s" : ""}: ${m.attachments.map((a) => `${a.filename} (id:${a.id})`).join(", ")} \u2014 use view_file to see]` : "";
        return `[target=${target} msg=${msgId} time=${time}${senderType}] @${m.sender_name}: ${m.content}${attachSuffix}`;
      }).join("\n");
      return {
        content: [{ type: "text", text: formatted }]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "list_server",
  "List all channels in this server, including which ones you have joined, plus all agents and humans. Use this to discover who and where you can message.",
  {},
  async () => {
    try {
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/server`,
        { method: "GET", headers: commonHeaders }
      );
      const data = await res.json();
      let text = "## Server\n\n";
      text += "### Channels\n";
      text += "Use `#channel-name` with send_message to post in a channel. `joined` means you currently belong to that channel.\n";
      if (data.channels?.length > 0) {
        for (const t of data.channels) {
          const status = t.joined ? "joined" : "not joined";
          text += t.description ? `  - #${t.name} [${status}] \u2014 ${t.description}\n` : `  - #${t.name} [${status}]\n`;
        }
      } else {
        text += "  (none)\n";
      }
      text += "\n### Agents\n";
      text += "Other AI agents in this server.\n";
      if (data.agents?.length > 0) {
        for (const a of data.agents) {
          text += `  - @${a.name} (${a.status})\n`;
        }
      } else {
        text += "  (none)\n";
      }
      text += "\n### Humans\n";
      text += 'To start a new DM: send_message(target="dm:@name"). To reply in an existing DM: reuse the target from received messages.\n';
      if (data.humans?.length > 0) {
        for (const u of data.humans) {
          text += `  - @${u.name}\n`;
        }
      } else {
        text += "  (none)\n";
      }
      return {
        content: [{ type: "text", text }]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "read_history",
  "Read message history for a channel, DM, or thread. Use the same target format: '#channel', 'dm:@name', '#channel:id' for threads, 'dm:@name:id' for DM threads. Supports pagination: use 'before' to load older messages, 'after' to load messages after a seq number (e.g. to catch up on unread).",
  {
    channel: z.string().describe("The target to read history from \u2014 e.g. '#general', 'dm:@richard', '#general:abcd1234', 'dm:@richard:abcd1234'"),
    limit: z.number().default(50).describe("Max number of messages to return (default 50, max 100)"),
    before: z.number().optional().describe("Return messages before this seq number (for backward pagination). Omit for latest messages."),
    after: z.number().optional().describe("Return messages after this seq number (for catching up on unread). Returns oldest-first.")
  },
  async ({ channel, limit, before, after }) => {
    try {
      const params = new URLSearchParams();
      params.set("channel", channel);
      params.set("limit", String(Math.min(limit, 100)));
      if (before) params.set("before", String(before));
      if (after) params.set("after", String(after));
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/history?${params}`,
        { method: "GET", headers: commonHeaders }
      );
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [
            { type: "text", text: `Error: ${data.error}` }
          ]
        };
      }
      if (!data.messages || data.messages.length === 0) {
        return {
          content: [
            { type: "text", text: "No messages in this channel." }
          ]
        };
      }
      const formatted = data.messages.map((m) => {
        const senderType = m.senderType === "agent" ? " type=agent" : "";
        const time = m.createdAt ? toLocalTime(m.createdAt) : "-";
        const msgId = m.id ? m.id.slice(0, 8) : "-";
        const attachSuffix = m.attachments?.length ? ` [${m.attachments.length} image${m.attachments.length > 1 ? "s" : ""}: ${m.attachments.map((a) => `${a.filename} (id:${a.id})`).join(", ")} \u2014 use view_file to see]` : "";
        return `[seq=${m.seq} msg=${msgId} time=${time}${senderType}] @${m.senderName}: ${m.content}${attachSuffix}`;
      }).join("\n");
      let footer = "";
      if (data.historyLimited) {
        footer = `\n\n--- ${data.historyLimitMessage || "Message history is limited on this plan."} ---`;
      } else if (data.has_more && data.messages.length > 0) {
        if (after) {
          const maxSeq = data.messages[data.messages.length - 1].seq;
          footer = `\n\n--- ${data.messages.length} messages shown. Use after=${maxSeq} to load more recent messages. ---`;
        } else {
          const minSeq = data.messages[0].seq;
          footer = `\n\n--- ${data.messages.length} messages shown. Use before=${minSeq} to load older messages. ---`;
        }
      }
      let header = `## Message History for ${channel} (${data.messages.length} messages)`;
      if (data.last_read_seq > 0 && !after && !before) {
        header += `\nYour last read position: seq ${data.last_read_seq}. Use read_history(channel="${channel}", after=${data.last_read_seq}) to see only unread messages.`;
      }
      return {
        content: [
          {
            type: "text",
            text: `${header}\n\n${formatted}${footer}`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "list_tasks",
  "List tasks on a channel's task board. Returns tasks with their number (#t1, #t2...), title, status, and assignee.",
  {
    channel: z.string().describe("The channel whose task board to view \u2014 e.g. '#engineering', '#proj-slock'"),
    status: z.enum(["all", "todo", "in_progress", "in_review", "done"]).default("all").describe("Filter by status (default: all)")
  },
  async ({ channel, status }) => {
    try {
      const params = new URLSearchParams();
      params.set("channel", channel);
      if (status !== "all") params.set("status", status);
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/tasks?${params}`,
        { method: "GET", headers: commonHeaders }
      );
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      if (!data.tasks || data.tasks.length === 0) {
        return {
          content: [{ type: "text", text: `No${status !== "all" ? ` ${status}` : ""} tasks in ${channel}.` }]
        };
      }
      const formatted = data.tasks.map((t) => {
        const assignee = t.claimedByName ? ` \u2192 @${t.claimedByName}` : "";
        const creator = t.createdByName ? ` (by @${t.createdByName})` : "";
        return `#t${t.taskNumber} [${t.status}] "${t.title}"${assignee}${creator}`;
      }).join("\n");
      return {
        content: [
          {
            type: "text",
            text: `## Task Board for ${channel} (${data.tasks.length} tasks)\n\n${formatted}`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "create_tasks",
  "Create one or more tasks on a channel's task board. Returns the created task numbers.",
  {
    channel: z.string().describe("The channel to create tasks in \u2014 e.g. '#engineering'"),
    tasks: z.array(
      z.object({
        title: z.string().describe("Task title")
      })
    ).describe("Array of tasks to create")
  },
  async ({ channel, tasks }) => {
    try {
      const res = await fetch(`${serverUrl}/internal/agent/${agentId}/tasks`, {
        method: "POST",
        headers: commonHeaders,
        body: JSON.stringify({ channel, tasks })
      });
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      const created = data.tasks.map((t) => `#t${t.taskNumber} "${t.title}"`).join("\n");
      return {
        content: [
          {
            type: "text",
            text: `Created ${data.tasks.length} task(s) in ${channel}:\n${created}`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "claim_tasks",
  "Claim one or more tasks by their number. Returns which claims succeeded and which failed (e.g. already claimed by someone else).",
  {
    channel: z.string().describe("The channel whose tasks to claim \u2014 e.g. '#engineering'"),
    task_numbers: z.array(z.number()).describe("Task numbers to claim (e.g. [1, 3, 5])")
  },
  async ({ channel, task_numbers }) => {
    try {
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/tasks/claim`,
        {
          method: "POST",
          headers: commonHeaders,
          body: JSON.stringify({ channel, task_numbers })
        }
      );
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      const lines = data.results.map((r) => {
        if (r.success) {
          return `#t${r.taskNumber}: claimed`;
        }
        return `#t${r.taskNumber}: FAILED \u2014 ${r.reason || "already claimed"}`;
      });
      const succeeded = data.results.filter((r) => r.success).length;
      const failed = data.results.length - succeeded;
      let summary = `${succeeded} claimed`;
      if (failed > 0) summary += `, ${failed} failed`;
      return {
        content: [
          {
            type: "text",
            text: `Claim results (${summary}):\n${lines.join("\n")}`
          }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "unclaim_task",
  "Release your claim on a task, setting it back to open.",
  {
    channel: z.string().describe("The channel \u2014 e.g. '#engineering'"),
    task_number: z.number().describe("The task number to unclaim (e.g. 3)")
  },
  async ({ channel, task_number }) => {
    try {
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/tasks/unclaim`,
        {
          method: "POST",
          headers: commonHeaders,
          body: JSON.stringify({ channel, task_number })
        }
      );
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      return {
        content: [
          { type: "text", text: `#t${task_number} unclaimed \u2014 now open.` }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

server.tool(
  "update_task_status",
  "Update a task's progress status. Valid transitions: todo\u2192in_progress, in_progress\u2192in_review, in_progress\u2192done, in_review\u2192done, in_review\u2192in_progress. You must be the assignee (except in_review\u2192done which anyone can do).",
  {
    channel: z.string().describe("The channel \u2014 e.g. '#engineering'"),
    task_number: z.number().describe("The task number to update (e.g. 3)"),
    status: z.enum(["todo", "in_progress", "in_review", "done"]).describe("The new status")
  },
  async ({ channel, task_number, status }) => {
    try {
      const res = await fetch(
        `${serverUrl}/internal/agent/${agentId}/tasks/update-status`,
        {
          method: "POST",
          headers: commonHeaders,
          body: JSON.stringify({ channel, task_number, status })
        }
      );
      const data = await res.json();
      if (!res.ok) {
        return {
          content: [{ type: "text", text: `Error: ${data.error}` }]
        };
      }
      return {
        content: [
          { type: "text", text: `#t${task_number} moved to ${status}.` }
        ]
      };
    } catch (err) {
      return {
        content: [{ type: "text", text: `Error: ${err.message}` }]
      };
    }
  }
);

var transport = new StdioServerTransport();
await server.connect(transport);
