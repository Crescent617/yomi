import * as api from "./api";
import type { TaggedContentBlock } from "./types";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";

export interface TabEntry {
  name: string;
  path: string;
  isDirectory: boolean;
  isFile: boolean;
}

export interface Tab {
  id: string;
  type: "chat" | "preview" | "edit";
  label: string;
  entry?: TabEntry;
  pinned?: boolean;
}

export interface ToolCall {
  id: string;
  toolName: string;
  status: "running" | "completed" | "failed" | "cancelled";
  arguments?: string;
  parsedArgs?: Record<string, unknown>;
  output?: string;
  error?: string;
  progress?: string;
  tokens?: number;
  elapsedMs?: number;
  folded?: boolean;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  contentBlocks?: TaggedContentBlock[];
  thinking?: { content: string; elapsedMs: number } | null;
  tools?: ToolCall[];
  error?: boolean;
}

export interface ProjectState {
  id: string;
  name: string;
  dir: string;
  createdAt: string;
  updatedAt: string;
}

export interface PendingPermission {
  reqId: string;
  toolName: string;
  toolArgs: string;
  toolLevel: string;
  reason: string;
}

export interface AskOption {
  label: string;
  description: string;
  preview?: string;
}

export interface AskQuestion {
  question: string;
  header: string;
  options: AskOption[];
  multiSelect: boolean;
}

export interface PendingAskUser {
  reqId: string;
  agentId: string;
  questions: AskQuestion[];
}

export interface QueuedInput {
  text: string;
  blocks?: TaggedContentBlock[];
}

export interface SessionState {
  id: string;
  projectPath: string;
  projectId?: string;
  alias?: string;
  messages: ChatMessage[];
  streaming: boolean;
  unread: number;
  checkpoints: unknown[];
  tabs: Tab[];
  activeTabId: string;
  pendingPermissions: PendingPermission[];
  pendingAskUser: PendingAskUser | null;
  queuedInput: QueuedInput | null;
  updatedAt: string;
  permissionLevel?: string;
  compacting?: boolean;
}

export const appState = $state({
  connectionStatus: "disconnected" as "connected" | "disconnected" | "connecting",
  currentTheme: "system" as "light" | "dark" | "system",
  sidebarCollapsed: false,
  rightPanelCollapsed: true,
  activePanel: "chat" as "chat" | "usage" | "config",
});

export const projectState = $state({
  projects: [] as ProjectState[],
  activeProjectId: null as string | null,
});

// Per-project session cursors for pagination
export const sessionCursors = $state(new Map<string, string | null>());

import { pushToast } from "./toast.svelte";

export function showNotification(
  text: string,
  level: "info" | "warn" | "error" | "success" = "info",
  durationMs = 4000
) {
  const typeMap: Record<string, "info" | "success" | "warning" | "error"> = {
    info: "info",
    success: "success",
    warn: "warning",
    error: "error",
  };
  pushToast(text, typeMap[level] ?? "info", durationMs);
}

export function sendDesktopNotification(title: string, body: string, sessionId?: string) {
  try {
    if (sessionId && typeof Notification !== "undefined") {
      try {
        const n = new Notification(title, { body, tag: sessionId });
        n.onclick = () => {
          getCurrentWindow().setFocus().catch(() => {});
          appState.activePanel = "chat";
          if (getSession(sessionId)) {
            setActiveSession(sessionId);
          }
        };
        return;
      } catch (webErr) {
        console.warn("Web Notification API failed, falling back to plugin:", webErr);
      }
    }
    sendNotification({ title, body });
  } catch (e) {
    console.error("Failed to send desktop notification:", e);
  }
}

export const sessionState = $state({
  sessions: [] as SessionState[],
  activeSessionId: null as string | null,
});

export const streamingMessages = $state<Record<string, ChatMessage[]>>({});

export function getDisplayMessages(sessionId: string): ChatMessage[] {
  const session = getSession(sessionId);
  if (!session) return [];
  const streamBuf = streamingMessages[sessionId] ?? [];
  if (streamBuf.length === 0) return session.messages;
  return [...session.messages, ...streamBuf];
}

export function getSession(id: string): SessionState | undefined {
  return sessionState.sessions.find((s) => s.id === id);
}

export function getActiveSession(): SessionState | null {
  return sessionState.activeSessionId
    ? getSession(sessionState.activeSessionId) ?? null
    : null;
}

export function setActiveSession(id: string | null) {
  const prevId = sessionState.activeSessionId;
  if (prevId && id !== prevId) {
    const prev = getSession(prevId);
    if (prev) prev.unread = 0;
  }
  sessionState.activeSessionId = id;
}

function upsertSession(session: SessionState) {
  const idx = sessionState.sessions.findIndex((s) => s.id === session.id);
  if (idx >= 0) {
    sessionState.sessions[idx] = session;
  } else {
    sessionState.sessions.push(session);
  }
}



export function openFileTab(
  session: SessionState,
  entry: TabEntry,
  type: "preview" | "edit"
) {
  const existing = session.tabs.find(
    (t) => t.type === type && t.entry?.path === entry.path
  );
  if (existing) {
    session.activeTabId = existing.id;
    return;
  }
  const newTab: Tab = {
    id: crypto.randomUUID(),
    type,
    label: entry.name,
    entry,
  };
  session.tabs = [...session.tabs, newTab];
  session.activeTabId = newTab.id;
}

export function closeTab(session: SessionState, tabId: string) {
  if (tabId === "chat") return;
  const idx = session.tabs.findIndex((t) => t.id === tabId);
  if (idx === -1) return;
  session.tabs = session.tabs.filter((t) => t.id !== tabId);
  if (session.activeTabId === tabId) {
    session.activeTabId = session.tabs[Math.min(idx, session.tabs.length - 1)]?.id ?? "chat";
  }
}

function extractId(raw: unknown): string {
  if (Array.isArray(raw) && raw.length > 0) raw = raw[0];
  else if (typeof raw === "object" && raw !== null) {
    const obj = raw as Record<string, unknown>;
    raw = obj["0"] ?? obj[0] ?? null;
  }
  return typeof raw === "string" && raw.length > 0 ? raw : crypto.randomUUID();
}

function normalizeRole(role: unknown): "user" | "tool" | "system" | "assistant" {
  if (role === "User" || role === "user") return "user";
  if (role === "tool" || role === "Tool") return "tool";
  if (role === "system" || role === "System") return "system";
  return "assistant";
}

import type { TaggedContentBlock } from "./types";

// ── State ────────────────────────────────────────────────────────────────

export interface ChatMessage {
  id?: unknown;
  role: unknown;
  content?: string | TaggedContentBlock[];
  toolCallId?: string;
  toolCalls?: RawToolCall[];
}

interface RawToolCall {
  id?: string;
  name?: string;
  toolName?: string;
  arguments?: string | Record<string, unknown>;
}

export function loadSessionMessages(sessionId: string, rawMessages: unknown[]) {
  const session = getSession(sessionId);
  if (!session) return;

  // First pass: collect all tool outputs from tool result messages
  const toolOutputs: Record<string, string> = {};
  const toolOutputByName: Record<string, string> = {};
  for (const m of rawMessages as RawMessage[]) {
    const role = normalizeRole(m.role);
    if (role !== "tool") continue;

    let output = "";
    if (Array.isArray(m.content)) {
      output = m.content.map((block: TaggedContentBlock) => {
        if (typeof block === "string") return block;
        return block.type === "text" && block.text ? block.text : "";
      }).join("");
    } else if (typeof m.content === "string") {
      output = m.content;
    }
    if (m.toolCallId) {
      toolOutputs[m.toolCallId] = output;
      const cleanId = m.toolCallId.replace(/^functions\./, "");
      if (cleanId !== m.toolCallId) {
        toolOutputs[cleanId] = output;
      }
      const match = m.toolCallId.match(/^functions\.(\w+):/);
      if (match) {
        toolOutputByName[match[1].toLowerCase()] = output;
      }
    }
    if (m.id && typeof m.id === "string") {
      toolOutputs[m.id] = output;
    }
  }

  // Second pass: build all messages with correct tool statuses
  const parsedMessages: ChatMessage[] = [];
  for (const m of rawMessages as RawMessage[]) {
    const role = normalizeRole(m.role);

    if (role === "tool") {
      continue;
    }

    if (role === "user") {
      let textContent = "";
      let blocks: TaggedContentBlock[] | undefined;
      if (Array.isArray(m.content)) {
        textContent = m.content.map((b: TaggedContentBlock) => b.type === "text" && b.text ? b.text : "").join("");
        // Keep blocks if there's any non-text content (e.g., images)
        if (m.content.some((b: TaggedContentBlock) => b.type !== "text")) {
          blocks = m.content;
        }
      } else if (typeof m.content === "string") {
        textContent = m.content;
      }
      parsedMessages.push({
        id: extractId(m.id),
        role: "user",
        content: textContent,
        contentBlocks: blocks,
        thinking: null,
        tools: [],
      });
    } else if (role === "system") {
      let text = "";
      if (Array.isArray(m.content)) {
        for (const block of m.content) {
          if (typeof block === "string") {
            text += block;
          } else if (block.type === "text" && block.text) {
            text += block.text;
          }
        }
      } else if (typeof m.content === "string") {
        text = m.content;
      }
      parsedMessages.push({
        id: extractId(m.id),
        role: "system",
        content: text,
        thinking: null,
        tools: [],
      });
    } else {
      // Assistant message
      let text = "";
      let thinking: { content: string; elapsedMs: number } | null = null;

      if (Array.isArray(m.content)) {
        for (const block of m.content) {
          if (typeof block === "string") {
            text += block;
          } else if (block.type === "text" || block.Text || block.text) {
            text += block.text || block.Text || "";
          } else if (block.type === "thinking" || block.Thinking) {
            thinking = {
              content: block.thinking || block.Thinking?.thinking || "",
              elapsedMs: 0,
            };
          }
        }
      } else if (typeof m.content === "string") {
        text = m.content;
      }

      // Extract toolCalls from the message
      const tools: ToolCall[] = [];
      if (Array.isArray(m.toolCalls)) {
        for (const tc of m.toolCalls) {
          const toolId = tc.id || "";
          const toolName = tc.name || tc.toolName || "";
          let args = "";
          if (tc.arguments) {
            args = typeof tc.arguments === "string" ? tc.arguments : JSON.stringify(tc.arguments);
          }
          const output =
            toolOutputs[toolId]
            || toolOutputs[toolId.replace(/^functions\./, "")]
            || toolOutputByName[toolName.toLowerCase()]
            || "";
          const hasOutput =
            output !== ""
            || toolId in toolOutputs
            || toolId.replace(/^functions\./, "") in toolOutputs
            || toolName.toLowerCase() in toolOutputByName;
          tools.push({
            id: toolId,
            toolName,
            status: hasOutput ? "completed" : "running",
            arguments: args,
            output: output || undefined,
            folded: true,
          });
        }
      }

      parsedMessages.push({
        id: extractId(m.id),
        role: "assistant",
        content: text,
        thinking,
        tools,
      });
    }
  }

  upsertSession({
    ...session,
    messages: parsedMessages,
  });
}

// ── Kernel event types (deserialized from Rust Event enum) ─────────────────

interface ChunkContent {
  text?: string;
  thinking?: { thinking?: string };
}

interface ModelChunk {
  chunk?: { content: ChunkContent };
  toolCallDelta?: { toolId: string; toolName: string; argumentsDelta?: string };
  completed?: Record<string, never>;
  error?: { message: string };
  compacting?: { active: boolean };
}

interface ToolStart {
  toolId: string;
  toolName: string;
  arguments?: string;
}

interface ToolEnd {
  toolId: string;
  toolName: string;
  isError: boolean;
  elapsedMs: number;
  contentBlocks?: TaggedContentBlock[];
}

interface ToolProgress {
  toolId: string;
  message?: string;
  tokens?: number;
}

interface ToolEvent {
  start?: ToolStart;
  end?: ToolEnd;
  progress?: ToolProgress;
}

interface AgentLifecycleStopped {
  state: {
    stopped: {
      reason:
        | { cancelled: { operation?: string } }
        | { failed: { error: string } }
        | { maxIterations: { reached: number } }
        | { completed: true };
    };
  };
}

interface AgentLifecycleRunning {
  state: "running";
}

type AgentLifecycle = AgentLifecycleRunning | AgentLifecycleStopped;

interface AgentEvent {
  lifecycle?: AgentLifecycle;
  error?: string;
  permissionRequest?: {
    reqId: string;
    toolName: string;
    toolArgs?: string;
    toolLevel?: string;
    reason?: string;
  };
  askUserQuestion?: {
    reqId: string;
    agentId: string;
    questions: AskQuestion[];
  };
}

interface SystemEvent {
  connected?: Record<string, never>;
  disconnected?: Record<string, never>;
  sessionSwitched?: { sessionId: string };
  titleUpdated?: { sessionId: string; title: string };
  rewound?: { sessionId: string; messages: RawMessage[] };
}

interface UserEvent {
  message?: {
    messageId: string;
    content: TaggedContentBlock[];
  };
}

type KernelEvent =
  | { model: ModelChunk }
  | { agent: AgentEvent }
  | { system: SystemEvent }
  | { tool: ToolEvent }
  | { user: UserEvent };

export function handleEvent(sessionId: string, rawEvent: unknown) {
  const session = getSession(sessionId);
  if (!session) return;

  const ev = rawEvent as KernelEvent;
  if ("model" in ev) {
    handleModelEvent(session, ev.model);
  } else if ("agent" in ev) {
    handleAgentEvent(session, ev.agent);
  } else if ("system" in ev) {
    handleSystemEvent(session, ev.system);
  } else if ("tool" in ev) {
    handleToolEvent(session, ev.tool);
  } else if ("user" in ev) {
    handleUserEvent(session, ev.user);
  }

  if (sessionState.activeSessionId !== sessionId) {
    session.unread++;
  }
}

/** Search all messages for a tool with the given id. */
function findToolById(session: SessionState, toolId: string): { msg: ChatMessage; tool: ToolCall } | null {
  const allMessages = [...session.messages, ...(streamingMessages[session.id] ?? [])];
  for (let i = allMessages.length - 1; i >= 0; i--) {
    const msg = allMessages[i];
    if (msg.role === "assistant" && msg.tools) {
      const tool = msg.tools.find((t) => t.id === toolId);
      if (tool) return { msg, tool };
    }
  }
  return null;
}

function handleModelEvent(session: SessionState, event: ModelChunk): boolean {
  if (event.chunk) {
    const chunk = event.chunk;
    const content = chunk.content;
    // Any chunk from the model means streaming is active
    if (!session.streaming) session.streaming = true;

    if (content?.text) {
      const text = content.text;
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (lastMsg && lastMsg.role === "assistant" && !lastMsg.thinking && (!lastMsg.tools || lastMsg.tools.length === 0)) {
        lastMsg.content += text;
      } else {
        buf.push({ id: crypto.randomUUID(), role: "assistant", content: text, thinking: null, tools: [] });
      }
      streamingMessages[session.id] = buf;
      return true;
    } else if (content?.thinking) {
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (lastMsg && lastMsg.role === "assistant" && !lastMsg.content && (!lastMsg.tools || lastMsg.tools.length === 0)) {
        if (!lastMsg.thinking) lastMsg.thinking = { content: "", elapsedMs: 0 };
        lastMsg.thinking.content += content.thinking.thinking ?? "";
      } else {
        buf.push({ id: crypto.randomUUID(), role: "assistant", content: "", thinking: { content: content.thinking.thinking ?? "", elapsedMs: 0 }, tools: [] });
      }
      streamingMessages[session.id] = buf;
      return true;
    }
    return true;
  } else if (event.toolCallDelta) {
    const delta = event.toolCallDelta;
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === delta.toolId);
    if (!tool) {
      tool = { id: delta.toolId, toolName: delta.toolName, status: "running", arguments: "", folded: true };
      lastMsg.tools.push(tool);
    } else if (delta.toolName) {
      tool.toolName = delta.toolName;
    }
    if (delta.argumentsDelta) {
      tool.arguments = (tool.arguments ?? "") + delta.argumentsDelta;
    }
    streamingMessages[session.id] = buf;
    if (!session.streaming) session.streaming = true;
    return true;
  } else if (event.compacting) {
    const active = event.compacting.active;
    session.compacting = active;
    if (!active) {
      // Compaction finished — reload messages to reflect compacted history
      api.getMessages(session.id).then((msgs) => {
        loadSessionMessages(session.id, msgs);
      }).catch((e: Error) => console.error("Failed to reload messages after compaction:", e));
    }
    return true;
  } else if (event.completed) {
    // Streaming chunks finished — merge buffer, but do not mutate session.messages
    // here. User messages are added only via UserEvent::Message from the kernel.
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      session.messages = [...session.messages, ...buf];
      streamingMessages[session.id] = [];
    }
    return true;
  } else if (event.error) {
    // Model-level streaming error — Stopped::Failed will end streaming
    return false;
  }
  return false;
}

function handleToolEvent(session: SessionState, event: ToolEvent): boolean {
  if (event.start) {
    const start = event.start;
    const found = findToolById(session, start.toolId);
    if (found) {
      found.tool.status = "running";
      if (start.arguments) found.tool.arguments = start.arguments;
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === start.toolId);
    if (!tool) {
      tool = {
        id: start.toolId,
        toolName: start.toolName,
        status: "running",
        arguments: start.arguments ?? "",
        folded: true,
      };
      lastMsg.tools.push(tool);
      showNotification(`Calling ${start.toolName}...`, "info", 2000);
    } else {
      tool.status = "running";
      if (start.arguments) tool.arguments = start.arguments;
    }
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.end) {
    const end = event.end;
    const found = findToolById(session, end.toolId);
    if (found) {
      found.tool.status = end.isError ? "failed" : "completed";
      found.tool.elapsedMs = end.elapsedMs;
      found.tool.output = end.contentBlocks
        ?.map((b: TaggedContentBlock) => {
          if (typeof b === "string") return b;
          return b.type === "text" && b.text ? b.text : "";
        })
        .join("");
      if (end.isError) {
        showNotification(`${end.toolName} failed`, "error", 4000);
      }
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === end.toolId);
    if (!tool) {
      // Tool 的 Start 事件可能丢失，从 End 重建
      tool = {
        id: end.toolId,
        toolName: end.toolName,
        status: end.isError ? "failed" : "completed",
        arguments: "",
        folded: true,
      };
      lastMsg.tools.push(tool);
    }
    tool.status = end.isError ? "failed" : "completed";
    tool.elapsedMs = end.elapsedMs;
    tool.output = end.contentBlocks
      ?.map((b: TaggedContentBlock) => {
        if (typeof b === "string") return b;
        return b.type === "text" && b.text ? b.text : "";
      })
      .join("");
    if (end.isError) {
      showNotification(`${end.toolName} failed`, "error", 4000);
    }
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.progress) {
    const progress = event.progress;
    const found = findToolById(session, progress.toolId);
    if (found) {
      found.tool.progress = progress.message;
      found.tool.tokens = progress.tokens;
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === progress.toolId);
    if (!tool) {
      tool = {
        id: progress.toolId,
        toolName: "",
        status: "running",
        arguments: "",
        folded: true,
      };
      lastMsg.tools.push(tool);
    }
    tool.progress = progress.message;
    tool.tokens = progress.tokens;
    streamingMessages[session.id] = buf;
    return true;
  }
  return false;
}

function handleAgentEvent(session: SessionState, event: AgentEvent): boolean {
  if (event.lifecycle) {
    const state = event.lifecycle.state;
    if (state === "running" && !session.streaming) {
      session.streaming = true;
      showNotification("AI is responding...", "info", 2000);
      return true;
    } else if (typeof state === "object") {
      if (state.stopped && session.streaming) {
        session.streaming = false;
        const buf = streamingMessages[session.id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[session.id] = [];
        }
        // Auto-send queued message when the agent actually stops.
        if (session.queuedInput) {
          const { text, blocks } = session.queuedInput;
          session.queuedInput = null;
          if (blocks && blocks.length > 0) {
            api.sendMessageBlocks(session.id, blocks).catch((e: Error) => console.error("Failed to send queued message:", e));
          } else {
            api.sendMessage(session.id, text).catch((e: Error) => console.error("Failed to send queued message:", e));
          }
        }
        const stopReason = state.stopped.reason;
        if ("cancelled" in stopReason) {
          const op = stopReason.cancelled.operation;
          const msg = op ? `Cancelled: ${op}` : "Cancelled";
          session.messages = [...session.messages, {
            id: crypto.randomUUID(),
            role: "system",
            content: msg,
            thinking: null,
            tools: [],
            error: true,
          }];
          showNotification(msg, "warning", 3000);
          sendDesktopNotification("Yomi", msg, session.id);
          return true;
        } else if ("failed" in stopReason) {
          const errorMsg = "Task failed: " + (stopReason.failed.error ?? "Unknown");
          session.messages = [...session.messages, {
            id: crypto.randomUUID(),
            role: "system",
            content: errorMsg,
            thinking: null,
            tools: [],
            error: true,
          }];
          showNotification(errorMsg, "error", 5000);
          sendDesktopNotification("Yomi", errorMsg, session.id);
          return true;
        } else if ("maxIterations" in stopReason) {
          const msg = `Max iterations reached (${stopReason.maxIterations.reached})`;
          showNotification(msg, "warning", 5000);
          sendDesktopNotification("Yomi", msg, session.id);
          return true;
        }
        // Completed: normal end
        sendDesktopNotification("Yomi", "Task completed", session.id);
        return true;
      }
    }
  } else if (event.error && session.streaming) {
    session.streaming = false;
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      session.messages = [...session.messages, ...buf];
      streamingMessages[session.id] = [];
    }
    const errorMsg = "Agent error: " + (event.error ?? "Unknown");
    session.messages = [...session.messages, {
      id: crypto.randomUUID(),
      role: "system",
      content: errorMsg,
      thinking: null,
      tools: [],
      error: true,
    }];
    showNotification(errorMsg, "error", 5000);
    sendDesktopNotification("Yomi", errorMsg, session.id);
    return true;
  } else if (event.permissionRequest) {
    const req = event.permissionRequest;
    session.pendingPermissions.push({
      reqId: req.reqId,
      toolName: req.toolName,
      toolArgs: req.toolArgs ?? "",
      toolLevel: req.toolLevel ?? "safe",
      reason: req.reason ?? "",
    });
    showNotification(`${req.toolName} needs approval`, "warn", 5000);
    return true;
  } else if (event.askUserQuestion) {
    const req = event.askUserQuestion;
    session.pendingAskUser = {
      reqId: req.reqId,
      agentId: req.agentId,
      questions: req.questions,
    };
    showNotification("Agent has a question for you", "info", 5000);
    return true;
  }
  return false;
}

function handleUserEvent(session: SessionState, event: UserEvent): boolean {
  if (event.message) {
    const msg = event.message;
    const content = msg.content
      ?.map((b: TaggedContentBlock) => {
        if (typeof b === "string") return b;
        return b.type === "text" && b.text ? b.text : "";
      })
      .join("") ?? "";
    const hasNonText = Array.isArray(msg.content) && msg.content.some((b: TaggedContentBlock) => typeof b !== "string" && b.type !== "text");
    session.messages = [
      ...session.messages,
      { id: msg.messageId, role: "user", content, contentBlocks: hasNonText ? msg.content : undefined, thinking: null, tools: [] },
    ];
    session.updatedAt = new Date().toISOString();
    // User message received → show streaming indicator immediately
    session.streaming = true;
    return true;
  }
  return false;
}

function handleSystemEvent(session: SessionState, event: SystemEvent): boolean {
  if (event.connected) {
    showNotification("Connected", "success", 2000);
    return true;
  } else if (event.disconnected) {
    showNotification("Disconnected", "error", 3000);
    return true;
  } else if (event.sessionSwitched) {
    return true;
  } else if (event.titleUpdated) {
    if (event.titleUpdated.sessionId !== session.id) return false;
    const idx = sessionState.sessions.findIndex((s) => s.id === session.id);
    if (idx >= 0) {
      sessionState.sessions[idx] = { ...sessionState.sessions[idx], alias: event.titleUpdated.title };
    }
    return true;
  } else if (event.rewound) {
    if (event.rewound.sessionId !== session.id) return false;
    // Clear any streaming buffer since history changed
    streamingMessages[session.id] = [];
    session.streaming = false;
    loadSessionMessages(session.id, event.rewound.messages);
    // Refresh checkpoints list after rewind
    api.getCheckpoints(session.id).then((cps) => {
      session.checkpoints = cps;
    }).catch((e: Error) => console.error("Failed to reload checkpoints after rewind:", e));
    showNotification("Session rewound", "info", 3000);
    return true;
  }
  return false;
}

export function updateConnectionStatus(status: "connected" | "disconnected" | "connecting") {
  appState.connectionStatus = status;
}