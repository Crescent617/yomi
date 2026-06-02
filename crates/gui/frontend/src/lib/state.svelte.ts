import * as api from "./api";

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
  thinking?: { content: string; elapsedMs: number } | null;
  tools?: ToolCall[];
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
  queuedInput: string | null;
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

export function addUserMessage(sessionId: string, text: string) {
  const session = getSession(sessionId);
  if (!session) return;
  const now = new Date().toISOString();
  upsertSession({
    ...session,
    messages: [
      ...session.messages,
      { id: crypto.randomUUID(), role: "user", content: text },
    ],
    updatedAt: now,
    alias: session.alias ?? text.slice(0, 20),
    streaming: true,
  });
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

// Helper for content blocks coming from the Rust backend
interface ContentBlock {
  Text?: string;
  text?: string;
  type?: string;
  thinking?: string;
  Thinking?: { thinking?: string };
}

// Raw message shape from the Rust backend
interface RawMessage {
  id?: unknown;
  role: unknown;
  content?: string | ContentBlock[];
  tool_call_id?: string;
  tool_calls?: RawToolCall[];
}

interface RawToolCall {
  id?: string;
  name?: string;
  tool_name?: string;
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
      output = m.content.map((block: ContentBlock) => {
        if (typeof block === "string") return block;
        if (block.Text) return block.Text;
        if (block.text) return block.text;
        return "";
      }).join("");
    } else if (typeof m.content === "string") {
      output = m.content;
    }
    if (m.tool_call_id) {
      toolOutputs[m.tool_call_id] = output;
      const cleanId = m.tool_call_id.replace(/^functions\./, "");
      if (cleanId !== m.tool_call_id) {
        toolOutputs[cleanId] = output;
      }
      const match = m.tool_call_id.match(/^functions\.(\w+):/);
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
      parsedMessages.push({
        id: extractId(m.id),
        role: "user",
        content: typeof m.content === "string"
          ? m.content
          : Array.isArray(m.content)
            ? m.content.map((b: ContentBlock) => b.Text || b.text || "").join("")
            : "",
        thinking: null,
        tools: [],
      });
    } else if (role === "system") {
      let text = "";
      if (Array.isArray(m.content)) {
        for (const block of m.content) {
          if (typeof block === "string") {
            text += block;
          } else if (block.type === "text" || block.Text || block.text) {
            text += block.text || block.Text || "";
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

      // Extract tool_calls from the message
      const tools: ToolCall[] = [];
      if (Array.isArray(m.tool_calls)) {
        for (const tc of m.tool_calls) {
          const toolId = tc.id || "";
          const toolName = tc.name || tc.tool_name || "";
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
  Text?: string;
  Thinking?: { thinking?: string };
}

interface ModelChunk {
  Chunk?: { content: ChunkContent };
  ToolCallDelta?: { tool_id: string; tool_name: string; arguments_delta?: string };
  Completed?: Record<string, never>;
  Error?: { message: string };
  Compacting?: { active: boolean };
}

interface ToolStart {
  tool_id: string;
  tool_name: string;
  arguments?: string;
}

interface ToolEnd {
  tool_id: string;
  tool_name: string;
  is_error: boolean;
  elapsed_ms: number;
  content_blocks?: ContentBlock[];
}

interface ToolProgress {
  tool_id: string;
  message?: string;
  tokens?: number;
}

interface ToolEvent {
  Start?: ToolStart;
  End?: ToolEnd;
  Progress?: ToolProgress;
}

interface AgentLifecycle {
  state: string | { TurnCompleted?: true; Stopped?: true };
}

interface AgentEvent {
  Lifecycle?: AgentLifecycle;
  Error?: { message?: string };
  PermissionRequest?: {
    req_id: string;
    tool_name: string;
    tool_args?: string;
    tool_level?: string;
    reason?: string;
  };
  AskUser?: {
    req_id: string;
    agent_id: string;
    questions: AskQuestion[];
  };
}

interface SystemEvent {
  Connected?: Record<string, never>;
  Disconnected?: Record<string, never>;
  SessionSwitched?: { session_id: string };
}

type KernelEvent =
  | { Model: ModelChunk }
  | { Agent: AgentEvent }
  | { System: SystemEvent }
  | { Tool: ToolEvent };

export function handleEvent(sessionId: string, rawEvent: unknown) {
  const session = getSession(sessionId);
  if (!session) return;

  const ev = rawEvent as KernelEvent;
  if ("Model" in ev) {
    handleModelEvent(session, ev.Model);
  } else if ("Agent" in ev) {
    handleAgentEvent(session, ev.Agent);
  } else if ("System" in ev) {
    handleSystemEvent(session, ev.System);
  } else if ("Tool" in ev) {
    handleToolEvent(session, ev.Tool);
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
  if (event.Chunk) {
    const chunk = event.Chunk;
    const content = chunk.content;
    // Any chunk from the model means streaming is active
    if (!session.streaming) session.streaming = true;

    if (content?.Text) {
      const text = content.Text;
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (lastMsg && lastMsg.role === "assistant" && !lastMsg.thinking && (!lastMsg.tools || lastMsg.tools.length === 0)) {
        lastMsg.content += text;
      } else {
        buf.push({ id: crypto.randomUUID(), role: "assistant", content: text, thinking: null, tools: [] });
      }
      streamingMessages[session.id] = buf;
      return true;
    } else if (content?.Thinking) {
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (lastMsg && lastMsg.role === "assistant" && !lastMsg.content && (!lastMsg.tools || lastMsg.tools.length === 0)) {
        if (!lastMsg.thinking) lastMsg.thinking = { content: "", elapsedMs: 0 };
        lastMsg.thinking.content += content.Thinking.thinking ?? "";
      } else {
        buf.push({ id: crypto.randomUUID(), role: "assistant", content: "", thinking: { content: content.Thinking.thinking ?? "", elapsedMs: 0 }, tools: [] });
      }
      streamingMessages[session.id] = buf;
      return true;
    }
    return true;
  } else if (event.ToolCallDelta) {
    const delta = event.ToolCallDelta;
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === delta.tool_id);
    if (!tool) {
      tool = { id: delta.tool_id, toolName: delta.tool_name, status: "running", arguments: "", folded: true };
      lastMsg.tools.push(tool);
    }
    if (delta.arguments_delta) {
      tool.arguments = (tool.arguments ?? "") + delta.arguments_delta;
    }
    streamingMessages[session.id] = buf;
    if (!session.streaming) session.streaming = true;
    return true;
  } else if (event.Compacting) {
    const active = event.Compacting.active;
    session.compacting = active;
    if (!active) {
      // Compaction finished — reload messages to reflect compacted history
      api.getMessages(session.id).then((msgs) => {
        loadSessionMessages(session.id, msgs);
      }).catch((e: Error) => console.error("Failed to reload messages after compaction:", e));
    }
    return true;
  } else if (event.Completed || event.Error) {
    if (session.streaming) {
      session.streaming = false;
      session.updatedAt = new Date().toISOString();
      // Merge streaming buffer into session messages
      const buf = streamingMessages[session.id] ?? [];
      if (buf.length > 0) {
        session.messages = [...session.messages, ...buf];
        streamingMessages[session.id] = [];
      }
      // Auto-send queued message when streaming ends
      if (session.queuedInput) {
        const text = session.queuedInput;
        session.queuedInput = null;
        session.messages = [...session.messages, { id: crypto.randomUUID(), role: "user", content: text, thinking: null, tools: [] }];
        api.sendMessage(session.id, text).catch((e: Error) => console.error("Failed to send queued message:", e));
      }
      return true;
    }
  }
  return false;
}

function handleToolEvent(session: SessionState, event: ToolEvent): boolean {
  if (event.Start) {
    const start = event.Start;
    const found = findToolById(session, start.tool_id);
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
    let tool = lastMsg.tools.find((t) => t.id === start.tool_id);
    if (!tool) {
      tool = {
        id: start.tool_id,
        toolName: start.tool_name,
        status: "running",
        arguments: start.arguments ?? "",
        folded: true,
      };
      lastMsg.tools.push(tool);
      showNotification(`Calling ${start.tool_name}...`, "info", 2000);
    } else {
      tool.status = "running";
      if (start.arguments) tool.arguments = start.arguments;
    }
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.End) {
    const end = event.End;
    const found = findToolById(session, end.tool_id);
    if (found) {
      found.tool.status = end.is_error ? "failed" : "completed";
      found.tool.elapsedMs = end.elapsed_ms;
      found.tool.output = end.content_blocks
        ?.map((b: ContentBlock) => {
          if (typeof b === "string") return b;
          if (b.Text) return b.Text;
          if (b.text) return b.text;
          return "";
        })
        .join("");
      if (end.is_error) {
        showNotification(`${end.tool_name} failed`, "error", 4000);
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
    let tool = lastMsg.tools.find((t) => t.id === end.tool_id);
    if (!tool) {
      // Tool 的 Start 事件可能丢失，从 End 重建
      tool = {
        id: end.tool_id,
        toolName: end.tool_name,
        status: end.is_error ? "failed" : "completed",
        arguments: "",
        folded: true,
      };
      lastMsg.tools.push(tool);
    }
    tool.status = end.is_error ? "failed" : "completed";
    tool.elapsedMs = end.elapsed_ms;
    tool.output = end.content_blocks
      ?.map((b: ContentBlock) => {
        if (typeof b === "string") return b;
        if (b.Text) return b.Text;
        if (b.text) return b.text;
        return "";
      })
      .join("");
    if (end.is_error) {
      showNotification(`${end.tool_name} failed`, "error", 4000);
    }
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.Progress) {
    const progress = event.Progress;
    const found = findToolById(session, progress.tool_id);
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
    let tool = lastMsg.tools.find((t) => t.id === progress.tool_id);
    if (!tool) {
      tool = {
        id: progress.tool_id,
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
  if (event.Lifecycle) {
    const state = event.Lifecycle.state;
    if (state === "Running" && !session.streaming) {
      session.streaming = true;
      showNotification("AI is responding...", "info", 2000);
      return true;
    } else if (typeof state === "object") {
      if ((state.TurnCompleted || state.Stopped) && session.streaming) {
        session.streaming = false;
        const buf = streamingMessages[session.id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[session.id] = [];
        }
        return true;
      }
    }
  } else if (event.Error && session.streaming) {
    session.streaming = false;
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      session.messages = [...session.messages, ...buf];
      streamingMessages[session.id] = [];
    }
    showNotification("Agent error: " + (event.Error.message ?? "Unknown"), "error", 5000);
    return true;
  } else if (event.PermissionRequest) {
    const req = event.PermissionRequest;
    session.pendingPermissions.push({
      reqId: req.req_id,
      toolName: req.tool_name,
      toolArgs: req.tool_args ?? "",
      toolLevel: req.tool_level ?? "safe",
      reason: req.reason ?? "",
    });
    showNotification(`${req.tool_name} needs approval`, "warn", 5000);
    return true;
  } else if (event.AskUser) {
    const req = event.AskUser;
    session.pendingAskUser = {
      reqId: req.req_id,
      agentId: req.agent_id,
      questions: req.questions,
    };
    showNotification("Agent has a question for you", "info", 5000);
    return true;
  }
  return false;
}

function handleSystemEvent(session: SessionState, event: SystemEvent): boolean {
  if (event.Connected) {
    showNotification("Connected", "success", 2000);
    return true;
  } else if (event.Disconnected) {
    showNotification("Disconnected", "error", 3000);
    return true;
  } else if (event.SessionSwitched) {
    return true;
  }
  return false;
}

export function updateConnectionStatus(status: "connected" | "disconnected" | "connecting") {
  appState.connectionStatus = status;
}
