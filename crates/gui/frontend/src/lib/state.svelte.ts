import * as api from "./api";
import type { GitInfo } from "./api";
import type { TaggedContentBlock } from "./types";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";

// 非活跃 session 自动 unsubscribe 延迟（60 秒）
const INACTIVE_UNSUBSCRIBE_DELAY = 60_000;

const pendingUnsubscribeTimers: Record<
  string,
  ReturnType<typeof setTimeout>
> = {};

export function scheduleUnsubscribe(session_id: string) {
  const session = getSession(session_id);
  if (!session) return;
  if (session.id === sessionState.activeSessionId) return; // 当前活跃的，不清理
  if (session.phase !== "idle" && session.phase !== "closed") return; // 正在运行，不清理

  cancelPendingUnsubscribe(session_id);

  pendingUnsubscribeTimers[session_id] = setTimeout(() => {
    api.unsubscribe(session_id).catch(() => {});
    delete pendingUnsubscribeTimers[session_id];
  }, INACTIVE_UNSUBSCRIBE_DELAY);
}

export function cancelPendingUnsubscribe(session_id: string) {
  const timer = pendingUnsubscribeTimers[session_id];
  if (timer) {
    clearTimeout(timer);
    delete pendingUnsubscribeTimers[session_id];
  }
}

export function unsubscribeAllInactive() {
  for (const session_id of Object.keys(pendingUnsubscribeTimers)) {
    clearTimeout(pendingUnsubscribeTimers[session_id]);
    delete pendingUnsubscribeTimers[session_id];
  }
  for (const session of sessionState.sessions) {
    if (session.id !== sessionState.activeSessionId) {
      api.unsubscribe(session.id).catch(() => {});
    }
  }
}

export interface TabEntry {
  name: string;
  path: string;
  is_directory: boolean;
  is_file: boolean;
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
  tool_name: string;
  status: "running" | "completed" | "failed" | "cancelled";
  arguments?: string;
  parsed_args?: Record<string, unknown>;
  output?: string;
  error?: string;
  progress?: string;
  tokens?: number;
  elapsed_ms?: number;
  folded?: boolean;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system" | "error";
  content: string;
  content_blocks?: TaggedContentBlock[];
  thinking?: { content: string; elapsed_ms: number } | null;
  tools?: ToolCall[];
  error?: boolean;
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  raw?: unknown;
  created_at?: string;
}

export interface ProjectState {
  id: string;
  name: string;
  dir: string;
  created_at: string;
  updated_at: string;
}

export interface PendingPermission {
  req_id: string;
  tool_name: string;
  tool_args: string;
  tool_level: string;
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
  multi_select: boolean;
}

export interface PendingAskUser {
  req_id: string;
  agent_id: string;
  questions: AskQuestion[];
}

export interface QueuedInput {
  text: string;
  blocks?: TaggedContentBlock[];
}

export interface SessionState {
  id: string;
  project_path: string;
  project_id?: string;
  alias?: string;
  messages: ChatMessage[];
  phase: string;
  unread: number;
  checkpoints: unknown[];
  tabs: Tab[];
  active_tab_id: string;
  pending_permissions: PendingPermission[];
  pending_ask_user: PendingAskUser | null;
  queued_input: QueuedInput | null;
  updated_at: string;
  permission_level?: string;
  /** derived from pinned_session table */
  is_pinned?: boolean;
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  git_info?: GitInfo | null;
  goal?: { description: string; status: string } | null;
  browserUrl?: string;
}

export const appState = $state({
  connectionStatus: "disconnected" as
    | "connected"
    | "disconnected"
    | "connecting",
  currentTheme: "system" as "light" | "dark" | "system",
  sidebarCollapsed: false,
  rightPanelCollapsed: true,
  activePanel: "chat" as "chat" | "usage" | "config" | "automation",
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
  durationMs = 4000,
) {
  const typeMap: Record<string, "info" | "success" | "warning" | "error"> = {
    info: "info",
    success: "success",
    warn: "warning",
    error: "error",
  };
  pushToast(text, typeMap[level] ?? "info", durationMs);
}

export function sendDesktopNotification(
  title: string,
  body: string,
  session_id?: string,
) {
  try {
    if (session_id && typeof Notification !== "undefined") {
      try {
        const n = new Notification(title, { body, tag: session_id });
        n.onclick = () => {
          getCurrentWindow()
            .setFocus()
            .catch(() => {});
          appState.activePanel = "chat";
          if (getSession(session_id)) {
            setActiveSession(session_id);
          }
        };
        return;
      } catch (webErr) {
        console.warn(
          "Web Notification API failed, falling back to plugin:",
          webErr,
        );
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

/** Metadata from the pinned_session table, keyed by session_id */
export const pinnedSessionMeta = $state(
  {} as Record<string, { pinned_at: string }>,
);

export const streamingMessages = $state<Record<string, ChatMessage[]>>({});

export function openBrowser(session_id: string, url: string) {
  const session = getSession(session_id);
  if (session) {
    session.browserUrl = url;
  }
}

export function closeBrowser(session_id: string) {
  const session = getSession(session_id);
  if (session) {
    session.browserUrl = undefined;
  }
}

export function getDisplayMessages(session_id: string): ChatMessage[] {
  const session = getSession(session_id);
  if (!session) return [];
  const streamBuf = streamingMessages[session_id] ?? [];
  if (streamBuf.length === 0) return session.messages;
  return [...session.messages, ...streamBuf];
}

export function getSession(id: string): SessionState | undefined {
  return sessionState.sessions.find((s) => s.id === id);
}

export function refreshCheckpoints(session_id: string) {
  api
    .getCheckpoints(session_id)
    .then((cps) => {
      const session = getSession(session_id);
      if (session) session.checkpoints = cps;
    })
    .catch((e: Error) => console.error("Failed to reload checkpoints:", e));
}

export function refreshSessions() {
  api
    .listSessions()
    .then((result) => {
      const existing = new Map(sessionState.sessions.map((s) => [s.id, s]));
      for (const s of result.sessions) {
        const current = existing.get(s.id);
        if (!current) {
          sessionState.sessions.push({
            id: s.id,
            project_path: s.project_path,
            project_id: s.project_id,
            alias: s.title,
            messages: [],
            phase: "idle",
            unread: 0,
            checkpoints: [],
            tabs: [],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_user: null,
            queued_input: null,
            updated_at: s.created_at,
            permission_level: s.auto_approve_level,
          });
        } else {
          current.alias = s.title ?? current.alias;
          current.updated_at = s.created_at ?? current.updated_at;
          current.permission_level =
            s.auto_approve_level ?? current.permission_level;
        }
      }
    })
    .catch((e: Error) => console.error("Failed to refresh sessions:", e));
}

export function loadPinnedSessions() {
  api
    .listPinnedSessions()
    .then((pinned) => {
      // Reset all session pinned flags
      for (const s of sessionState.sessions) {
        s.is_pinned = false;
      }
      // Clear meta object without reassigning (Svelte 5 restriction)
      for (const key of Object.keys(pinnedSessionMeta)) {
        delete pinnedSessionMeta[key];
      }

      for (const p of pinned) {
        pinnedSessionMeta[p.session_id] = {
          pinned_at: p.pinned_at,
        };

        let session = getSession(p.session_id);
        if (!session) {
          session = {
            id: p.session_id,
            project_path: "",
            project_id: p.project_id,
            alias: p.title ?? "Untitled",
            messages: [],
            phase: "idle",
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_user: null,
            queued_input: null,
            updated_at: p.updated_at,
            is_pinned: true,
          };
          sessionState.sessions.push(session);
        } else {
          session.is_pinned = true;
          session.alias = p.title ?? session.alias ?? "Untitled";
          session.updated_at = p.updated_at ?? session.updated_at;
          session.project_id = p.project_id ?? session.project_id;
        }
      }
    })
    .catch((e: Error) => console.error("Failed to load pinned sessions:", e));
}

export function getActiveSession(): SessionState | null {
  return sessionState.activeSessionId
    ? (getSession(sessionState.activeSessionId) ?? null)
    : null;
}

export function setActiveSession(id: string | null) {
  const prevId = sessionState.activeSessionId;
  if (prevId && id !== prevId) {
    const prev = getSession(prevId);
    if (prev) prev.unread = 0;
    // 旧 session 进入非活跃，如果不在 streaming，延迟 unsubscribe
    scheduleUnsubscribe(prevId);
  }
  // 新 session 被激活，取消可能存在的 pending unsubscribe
  if (id) {
    cancelPendingUnsubscribe(id);
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

export function syncSessionStatus(
  session_id: string,
  status: { phase: string },
) {
  const session = getSession(session_id);
  if (!session) return;
  session.phase = status.phase;
}

export function openFileTab(
  session: SessionState,
  entry: TabEntry,
  type: "preview" | "edit",
) {
  const existing = session.tabs.find(
    (t) => t.type === type && t.entry?.path === entry.path,
  );
  if (existing) {
    session.active_tab_id = existing.id;
    return;
  }
  const newTab: Tab = {
    id: crypto.randomUUID(),
    type,
    label: entry.name,
    entry,
  };
  session.tabs = [...session.tabs, newTab];
  session.active_tab_id = newTab.id;
}

export function closeTab(session: SessionState, tabId: string) {
  if (tabId === "chat") return;
  const idx = session.tabs.findIndex((t) => t.id === tabId);
  if (idx === -1) return;
  session.tabs = session.tabs.filter((t) => t.id !== tabId);
  if (session.active_tab_id === tabId) {
    session.active_tab_id =
      session.tabs[Math.min(idx, session.tabs.length - 1)]?.id ?? "chat";
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

function normalizeRole(
  role: unknown,
): "user" | "tool" | "system" | "assistant" | "error" {
  if (role === "User" || role === "user") return "user";
  if (role === "tool" || role === "Tool") return "tool";
  if (role === "system" || role === "System") return "system";
  if (role === "error" || role === "Error") return "error";
  return "assistant";
}

export interface RawMessage {
  id?: unknown;
  role: unknown;
  content?: string | TaggedContentBlock[];
  tool_call_id?: string;
  tool_calls?: RawToolCall[];
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  created_at?: string;
}

interface RawToolCall {
  id?: string;
  name?: string;
  tool_name?: string;
  arguments?: string | Record<string, unknown>;
}

export function loadSessionMessages(
  session_id: string,
  rawMessages: unknown[],
) {
  const session = getSession(session_id);
  if (!session) return;

  // First pass: collect all tool outputs from tool result messages
  const toolOutputs: Record<string, string> = {};
  const toolOutputByName: Record<string, string> = {};
  for (const m of rawMessages as RawMessage[]) {
    const role = normalizeRole(m.role);
    if (role !== "tool") continue;

    let output = "";
    if (Array.isArray(m.content)) {
      output = m.content
        .map((block: TaggedContentBlock) => {
          if (typeof block === "string") return block;
          return block.type === "text" && block.text ? block.text : "";
        })
        .join("");
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
      let textContent = "";
      let blocks: TaggedContentBlock[] | undefined;
      if (Array.isArray(m.content)) {
        textContent = m.content
          .map((b: TaggedContentBlock) =>
            b.type === "text" && b.text ? b.text : "",
          )
          .join("");
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
        content_blocks: blocks,
        thinking: null,
        tools: [],
        raw: m,
        created_at: m.created_at,
      });
    } else if (role === "system" || role === "error") {
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
        role: role as "system" | "error",
        content: text,
        thinking: null,
        tools: [],
        raw: m,
        created_at: m.created_at,
      });
    } else {
      // Assistant message
      let text = "";
      let thinking: { content: string; elapsed_ms: number } | null = null;

      if (Array.isArray(m.content)) {
        for (const block of m.content) {
          if (typeof block === "string") {
            text += block;
          } else if (
            block.type === "text" ||
            (block as unknown as Record<string, unknown>).text
          ) {
            text +=
              ((block as unknown as Record<string, unknown>).text as string) ||
              "";
          } else if (
            block.type === "thinking" ||
            (block as unknown as Record<string, unknown>).thinking
          ) {
            thinking = {
              content:
                ((block as unknown as Record<string, unknown>)
                  .thinking as string) || "",
              elapsed_ms: 0,
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
          const tool_id = tc.id || "";
          const tool_name = tc.name || tc.tool_name || "";
          let args = "";
          if (tc.arguments) {
            args =
              typeof tc.arguments === "string"
                ? tc.arguments
                : JSON.stringify(tc.arguments);
          }
          const output =
            toolOutputs[tool_id] ||
            toolOutputs[tool_id.replace(/^functions\./, "")] ||
            toolOutputByName[tool_name.toLowerCase()] ||
            "";
          const hasOutput =
            output !== "" ||
            tool_id in toolOutputs ||
            tool_id.replace(/^functions\./, "") in toolOutputs ||
            tool_name.toLowerCase() in toolOutputByName;
          tools.push({
            id: tool_id,
            tool_name,
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
        token_usage: m.token_usage
          ? {
              prompt_tokens: m.token_usage.prompt_tokens,
              completion_tokens: m.token_usage.completion_tokens,
              total_tokens: m.token_usage.total_tokens,
            }
          : undefined,
        raw: m,
        created_at: m.created_at,
      });
    }
  }

  // Find the latest token usage from assistant messages (aligns with TUI logic)
  let latestTokenUsage = session.token_usage;
  for (let i = parsedMessages.length - 1; i >= 0; i--) {
    const msg = parsedMessages[i];
    if (msg.role === "assistant" && msg.token_usage) {
      latestTokenUsage = msg.token_usage;
      break;
    }
  }

  upsertSession({
    ...session,
    messages: parsedMessages,
    token_usage: latestTokenUsage,
  });
}

// ── Kernel event types (deserialized from Rust Event enum) ─────────────────

interface ChunkContent {
  text?: string;
  thinking?: { thinking?: string; signature?: string };
  redacted_thinking?: null;
}

interface ModelChunk {
  request?: { agent_id: string; message_id: string; message_count: number };
  chunk?: { agent_id: string; message_id: string; content: ChunkContent };
  tool_call_delta?: {
    agent_id: string;
    message_id: string;
    tool_id: string;
    tool_name: string;
    arguments_delta: string;
  };
  completed?: { agent_id: string; message_id: string };
  error?: { agent_id: string; message_id: string; error: string };
  fallback?: { agent_id: string; message_id: string; from: string; to: string };
  compacting?: { agent_id: string; active: boolean };
  token_usage?: {
    agent_id: string;
    message_id: string;
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
    context_window: number;
  };
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
  content_blocks?: TaggedContentBlock[];
}

interface ToolProgress {
  tool_id: string;
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
        | { max_iterations: { reached: number } }
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
  error?: {
    agent_id: string;
    phase: string;
    error: string;
    is_recoverable: boolean;
  };
  permission_request?: {
    req_id: string;
    tool_name: string;
    tool_args?: string;
    tool_level?: string;
    reason?: string;
  };
  ask_user_question?: {
    req_id: string;
    agent_id: string;
    questions: AskQuestion[];
  };
  retrying?: {
    agent_id: string;
    attempt: number;
    max_attempts: number;
    reason: string;
  };
}

interface SystemEvent {
  connected?: Record<string, never>;
  connection_lost?: Record<string, never>;
  shutdown?: { error?: string };
  session_switched?: { session_id: string };
  title_updated?: { session_id: string; title: string };
  rewound?: { session_id: string; messages: RawMessage[] };
  goal_updated?: { session_id: string; description: string; status: string };
  goal_stopped?: { session_id: string };
}

interface UserEvent {
  message?: {
    message_id: string;
    content: TaggedContentBlock[];
  };
}

type KernelEvent =
  | { model: ModelChunk }
  | { agent: AgentEvent }
  | { system: SystemEvent }
  | { tool: ToolEvent }
  | { user: UserEvent };

export function handleEvent(session_id: string, rawEvent: unknown) {
  let session = getSession(session_id);
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

  // Re-fetch session in case an event handler replaced it in sessionState.sessions
  session = getSession(session_id) ?? session;
  if (sessionState.activeSessionId !== session_id) {
    session.unread++;
  }
}

/** Mark a session as streaming and cancel any pending unsubscribe. */
function startStreaming(session: SessionState) {
  if (session.phase !== "streaming") {
    session.phase = "streaming";
  }
  cancelPendingUnsubscribe(session.id);
}

/** Schedule unsubscribe if the session is not currently active. */
function scheduleUnsubscribeIfInactive(session: SessionState) {
  if (session.id !== sessionState.activeSessionId) {
    scheduleUnsubscribe(session.id);
  }
}

/** Search all messages for a tool with the given id. */
function findToolById(
  session: SessionState,
  tool_id: string,
): { msg: ChatMessage; tool: ToolCall } | null {
  const allMessages = [
    ...session.messages,
    ...(streamingMessages[session.id] ?? []),
  ];
  for (let i = allMessages.length - 1; i >= 0; i--) {
    const msg = allMessages[i];
    if (msg.role === "assistant" && msg.tools) {
      const tool = msg.tools.find((t) => t.id === tool_id);
      if (tool) return { msg, tool };
    }
  }
  return null;
}

function handleModelEvent(session: SessionState, event: ModelChunk): boolean {
  if (event.token_usage) {
    const u = event.token_usage;
    session.token_usage = {
      prompt_tokens: u.prompt_tokens,
      completion_tokens: u.completion_tokens,
      total_tokens: u.total_tokens,
    };
    return true;
  }

  if (event.chunk) {
    const chunk = event.chunk;
    const content = chunk.content;
    startStreaming(session);

    if (content?.text) {
      const text = content.text;
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (
        lastMsg &&
        lastMsg.role === "assistant" &&
        !lastMsg.thinking &&
        (!lastMsg.tools || lastMsg.tools.length === 0)
      ) {
        lastMsg.content += text;
      } else {
        buf.push({
          id: crypto.randomUUID(),
          role: "assistant",
          content: text,
          thinking: null,
          tools: [],
          created_at: new Date().toISOString(),
        });
      }
      streamingMessages[session.id] = buf;
      return true;
    } else if (content?.thinking) {
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (
        lastMsg &&
        lastMsg.role === "assistant" &&
        !lastMsg.content &&
        (!lastMsg.tools || lastMsg.tools.length === 0)
      ) {
        if (!lastMsg.thinking)
          lastMsg.thinking = { content: "", elapsed_ms: 0 };
        lastMsg.thinking.content += content.thinking.thinking ?? "";
      } else {
        buf.push({
          id: crypto.randomUUID(),
          role: "assistant",
          content: "",
          thinking: { content: content.thinking.thinking ?? "", elapsed_ms: 0 },
          tools: [],
          created_at: new Date().toISOString(),
        });
      }
      streamingMessages[session.id] = buf;
      return true;
    } else if (content?.redacted_thinking !== undefined) {
      // Redacted thinking - no content to display, just mark streaming
      return true;
    }
    return true;
  } else if (event.tool_call_delta) {
    const delta = event.tool_call_delta;
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
        thinking: null,
        tools: [],
        created_at: new Date().toISOString(),
      };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === delta.tool_id);
    if (!tool) {
      tool = {
        id: delta.tool_id,
        tool_name: delta.tool_name,
        status: "running",
        arguments: "",
        folded: true,
      };
      lastMsg.tools.push(tool);
    } else if (delta.tool_name) {
      tool.tool_name = delta.tool_name;
    }
    if (delta.arguments_delta) {
      tool.arguments = (tool.arguments ?? "") + delta.arguments_delta;
    }
    streamingMessages[session.id] = buf;
    startStreaming(session);
    return true;
  } else if (event.compacting) {
    const active = event.compacting.active;
    if (active) {
      session.phase = "compacting";
      cancelPendingUnsubscribe(session.id);
    } else if (session.phase === "compacting") {
      session.phase = "idle";
    }
    if (!active) {
      // Compaction finished — reload messages to reflect compacted history
      api
        .getMessages(session.id)
        .then((msgs) => {
          loadSessionMessages(session.id, msgs);
        })
        .catch((e: Error) =>
          console.error("Failed to reload messages after compaction:", e),
        );
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
    const err = event.error;
    showNotification(`Model error: ${err.error}`, "error", 3000);
    return false;
  } else if (event.request) {
    // Model request started - no UI action needed
    return true;
  } else if (event.fallback) {
    const fb = event.fallback;
    showNotification(`Fallback from ${fb.from} to ${fb.to}`, "info", 2000);
    return true;
  }
  return false;
}

function maybeRefreshGitInfo(session: SessionState) {
  if (!session.project_path || session.id !== sessionState.activeSessionId)
    return;
  const { id, project_path } = session;
  api
    .getGitInfo(project_path)
    .then((info) => {
      const current = getSession(id);
      if (current && current.id === sessionState.activeSessionId) {
        current.git_info = info;
      }
    })
    .catch(() => {
      const current = getSession(id);
      if (current && current.id === sessionState.activeSessionId) {
        current.git_info = null;
      }
    });
}

function handleToolEvent(session: SessionState, event: ToolEvent): boolean {
  if (event.start) {
    const start = event.start;
    const found = findToolById(session, start.tool_id);
    if (found) {
      found.tool.status = "running";
      if (start.arguments) found.tool.arguments = start.arguments;
      session.phase = "executing_tool";
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
        thinking: null,
        tools: [],
        created_at: new Date().toISOString(),
      };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === start.tool_id);
    if (!tool) {
      tool = {
        id: start.tool_id,
        tool_name: start.tool_name,
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
    session.phase = "executing_tool";
    return true;
  } else if (event.end) {
    const end = event.end;
    const found = findToolById(session, end.tool_id);
    if (found) {
      found.tool.status = end.is_error ? "failed" : "completed";
      found.tool.elapsed_ms = end.elapsed_ms;
      found.tool.output = end.content_blocks
        ?.map((b: TaggedContentBlock) => {
          if (typeof b === "string") return b;
          return b.type === "text" && b.text ? b.text : "";
        })
        .join("");
      maybeRefreshGitInfo(session);
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
        thinking: null,
        tools: [],
        created_at: new Date().toISOString(),
      };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === end.tool_id);
    if (!tool) {
      // Tool 的 Start 事件可能丢失，从 End 重建
      tool = {
        id: end.tool_id,
        tool_name: end.tool_name,
        status: end.is_error ? "failed" : "completed",
        arguments: "",
        folded: true,
      };
      lastMsg.tools.push(tool);
    }
    tool.status = end.is_error ? "failed" : "completed";
    tool.elapsed_ms = end.elapsed_ms;
    tool.output = end.content_blocks
      ?.map((b: TaggedContentBlock) => {
        if (typeof b === "string") return b;
        return b.type === "text" && b.text ? b.text : "";
      })
      .join("");
    streamingMessages[session.id] = buf;
    maybeRefreshGitInfo(session);
    return true;
  } else if (event.progress) {
    const progress = event.progress;
    const found = findToolById(session, progress.tool_id);
    if (found) {
      found.tool.progress = progress.message;
      found.tool.tokens = progress.tokens;
      return true;
    }
    const buf = streamingMessages[session.id] ?? [];
    let lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    if (!lastMsg || lastMsg.role !== "assistant") {
      lastMsg = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
        thinking: null,
        tools: [],
        created_at: new Date().toISOString(),
      };
      buf.push(lastMsg);
    }
    if (!lastMsg.tools) lastMsg.tools = [];
    let tool = lastMsg.tools.find((t) => t.id === progress.tool_id);
    if (!tool) {
      tool = {
        id: progress.tool_id,
        tool_name: "",
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
    if (state === "running" && session.phase !== "streaming") {
      startStreaming(session);
      return true;
    } else if (typeof state === "object") {
      if (
        state.stopped &&
        (session.phase === "streaming" || session.phase === "executing_tool")
      ) {
        session.phase = "idle";
        scheduleUnsubscribeIfInactive(session);
        const buf = streamingMessages[session.id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[session.id] = [];
        }
        // Refresh checkpoints after a turn completes
        refreshCheckpoints(session.id);
        // Auto-send queued message when the agent actually stops.
        if (session.queued_input) {
          const { text, blocks } = session.queued_input;
          session.queued_input = null;
          if (blocks && blocks.length > 0) {
            api
              .sendMessageBlocks(session.id, blocks)
              .catch((e: Error) =>
                console.error("Failed to send queued message:", e),
              );
          } else {
            api
              .sendMessage(session.id, text)
              .catch((e: Error) =>
                console.error("Failed to send queued message:", e),
              );
          }
        }
        const stopReason = state.stopped.reason;
        if ("cancelled" in stopReason) {
          const op = stopReason.cancelled.operation;
          const msg = op ? `Cancelled: ${op}` : "Cancelled";
          session.messages = [
            ...session.messages,
            {
              id: crypto.randomUUID(),
              role: "error",
              content: msg,
              thinking: null,
              tools: [],
            },
          ];
          showNotification(msg, "warn", 3000);
          sendDesktopNotification("Yomi", msg, session.id);
          return true;
        } else if ("failed" in stopReason) {
          const errorMsg =
            "Task failed: " + (stopReason.failed.error ?? "Unknown");
          session.messages = [
            ...session.messages,
            {
              id: crypto.randomUUID(),
              role: "error",
              content: errorMsg,
              thinking: null,
              tools: [],
            },
          ];
          showNotification(errorMsg, "warn", 5000);
          sendDesktopNotification("Yomi", errorMsg, session.id);
          return true;
        } else if ("max_iterations" in stopReason) {
          const msg = `Max iterations reached (${stopReason.max_iterations.reached})`;
          session.messages = [
            ...session.messages,
            {
              id: crypto.randomUUID(),
              role: "error",
              content: msg,
              thinking: null,
              tools: [],
            },
          ];
          showNotification(msg, "warn", 5000);
          sendDesktopNotification("Yomi", msg, session.id);
          return true;
        }
        // Completed: normal end
        sendDesktopNotification("Yomi", "Task completed", session.id);
        return true;
      }
    }
  } else if (
    event.error &&
    session.phase !== "idle" &&
    session.phase !== "closed"
  ) {
    session.phase = "idle";
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      session.messages = [...session.messages, ...buf];
      streamingMessages[session.id] = [];
    }
    const errorStr = event.error.error ?? "Unknown";
    const errorMsg = "Agent error: " + errorStr;
    session.messages = [
      ...session.messages,
      {
        id: crypto.randomUUID(),
        role: "error",
        content: errorMsg,
        thinking: null,
        tools: [],
      },
    ];
    const level = event.error.is_recoverable ? "warn" : "error";
    showNotification(errorMsg, level, 5000);
    sendDesktopNotification("Yomi", errorMsg, session.id);
    scheduleUnsubscribeIfInactive(session);
    return true;
  } else if (event.retrying) {
    const retry = event.retrying;
    const msg = `Agent retrying (${retry.attempt}/${retry.max_attempts})`;
    session.messages = [
      ...session.messages,
      {
        id: crypto.randomUUID(),
        role: "error",
        content: msg,
        thinking: null,
        tools: [],
      },
    ];
    showNotification(msg, "warn", 3000);
    return true;
  } else if (event.permission_request) {
    const req = event.permission_request;
    session.pending_permissions.push({
      req_id: req.req_id,
      tool_name: req.tool_name,
      tool_args: req.tool_args ?? "",
      tool_level: req.tool_level ?? "safe",
      reason: req.reason ?? "",
    });
    showNotification(`${req.tool_name} needs approval`, "warn", 5000);
    return true;
  } else if (event.ask_user_question) {
    const req = event.ask_user_question;
    session.pending_ask_user = {
      req_id: req.req_id,
      agent_id: req.agent_id,
      questions: req.questions,
    };
    showNotification("Agent has a question for you", "info", 5000);
    sendDesktopNotification("Yomi", "Agent has a question for you", session.id);
    return true;
  }
  return false;
}

function handleUserEvent(session: SessionState, event: UserEvent): boolean {
  if (event.message) {
    const msg = event.message;
    const content =
      msg.content
        ?.map((b: TaggedContentBlock) => {
          if (typeof b === "string") return b;
          return b.type === "text" && b.text ? b.text : "";
        })
        .join("") ?? "";
    const hasNonText =
      Array.isArray(msg.content) &&
      msg.content.some(
        (b: TaggedContentBlock) => typeof b !== "string" && b.type !== "text",
      );

    session.messages.push({
      id: msg.message_id,
      role: "user",
      content,
      content_blocks: hasNonText ? msg.content : undefined,
      thinking: null,
      tools: [],
      created_at: new Date().toISOString(),
    });
    session.updated_at = new Date().toISOString();
    startStreaming(session);
    return true;
  }
  return false;
}

function handleSystemEvent(session: SessionState, event: SystemEvent): boolean {
  if (event.connected) {
    showNotification("Connected", "success", 2000);
    return true;
  } else if (event.connection_lost) {
    showNotification("Connection lost", "warn", 3000);
    return true;
  } else if (event.shutdown) {
    if (event.shutdown.error) {
      showNotification(`Session error: ${event.shutdown.error}`, "error", 5000);
    } else {
      showNotification("Session ended", "info", 2000);
    }
    session.phase = "idle";
    streamingMessages[session.id] = [];
    scheduleUnsubscribeIfInactive(session);
    return true;
  } else if (event.session_switched) {
    return true;
  } else if (event.title_updated) {
    if (event.title_updated.session_id !== session.id) return false;
    const idx = sessionState.sessions.findIndex((s) => s.id === session.id);
    if (idx >= 0) {
      sessionState.sessions[idx] = {
        ...sessionState.sessions[idx],
        alias: event.title_updated.title,
      };
    }
    return true;
  } else if (event.rewound) {
    if (event.rewound.session_id !== session.id) return false;
    // Clear any streaming buffer since history changed
    streamingMessages[session.id] = [];
    session.phase = "idle";
    scheduleUnsubscribeIfInactive(session);
    loadSessionMessages(session.id, event.rewound.messages);
    // Refresh checkpoints list after rewind
    refreshCheckpoints(session.id);
    showNotification("Session rewound", "info", 3000);
    return true;
  } else if (event.goal_updated) {
    if (event.goal_updated.session_id !== session.id) return false;
    session.goal = {
      description: event.goal_updated.description,
      status: event.goal_updated.status,
    };
    return true;
  } else if (event.goal_stopped) {
    if (event.goal_stopped.session_id !== session.id) return false;
    session.goal = null;
    return true;
  }
  return false;
}

export function updateConnectionStatus(
  status: "connected" | "disconnected" | "connecting",
) {
  appState.connectionStatus = status;
}
