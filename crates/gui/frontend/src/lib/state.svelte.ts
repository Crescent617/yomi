import * as api from "./api";
import type { GitInfo, SessionMessage } from "./api";
import type { TaggedContentBlock } from "./types";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const inFlightActivations = new Map<string, Promise<void>>();

/** Start listening for kernel notifications and update non-active sessions. */
export function startNotificationListener(): Promise<() => void> {
  return listen(
    "kernel:noti",
    (e: {
      payload: {
        state_changed?: { session_id: string; status: string };
        title_updated?: { session_id: string; title: string };
        connection_lost?: { session_id: string };
      };
    }) => {
      const payload = e.payload;
      if (payload.state_changed) {
        const { session_id, status } = payload.state_changed;
        const session = getSession(session_id);
        if (!session) return;
        session.phase = status;
        session.is_running = status !== "idle" && status !== "closed";
      }
      if (payload.title_updated) {
        const { session_id, title } = payload.title_updated;
        const session = getSession(session_id);
        if (!session) return;
        session.alias = title;
      }
      if (payload.connection_lost) {
        showNotification("Connection lost", "warn", 3000);
      }
    },
  );
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
  status: "pending" | "running" | "completed" | "failed" | "cancelled";
  arguments?: string;
  parsed_args?: Record<string, unknown>;
  output?: string;
  error?: string;
  progress?: string;
  tokens?: number;
  elapsed_ms?: number;
  subagent_session_id?: string;
}

// ── Helper: extract plain text from content blocks ─────────────────────────

export function textFromBlocks(blocks: TaggedContentBlock[] | unknown): string {
  if (!Array.isArray(blocks)) return "";
  return blocks
    .filter(
      (b): b is TaggedContentBlock & { text: string } =>
        b.type === "text" && typeof b.text === "string",
    )
    .map((b) => b.text)
    .join("");
}

export function hasText(blocks: TaggedContentBlock[] | unknown): boolean {
  if (!Array.isArray(blocks)) return false;
  return blocks.some((b) => b.type === "text" && b.text && b.text.length > 0);
}

export function findThinking(
  blocks: TaggedContentBlock[] | unknown,
): { content: string; elapsed_ms: number } | null {
  if (!Array.isArray(blocks)) return null;
  const block = blocks.find((b) => b.type === "thinking" && b.thinking);
  if (!block || !block.thinking) return null;
  return { content: block.thinking, elapsed_ms: 0 };
}

// ── Message types: match backend SessionMessage shape, minimal conversion ────

interface BaseMessage {
  id: string;
  created_at: string;
}

export interface UserMessage extends BaseMessage {
  type: "user";
  content: TaggedContentBlock[];
}

export interface BotMessage extends BaseMessage {
  type: "assistant";
  content: TaggedContentBlock[];
  tool_calls?: { id: string; name: string; arguments: string }[];
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

export interface ToolMessage extends BaseMessage {
  type: "tool";
  tool_call_id: string;
  tool_name: string;
  status: "pending" | "running" | "completed" | "failed" | "cancelled";
  arguments: string;
  result: TaggedContentBlock[];
  elapsed_ms?: number;
  subagent_session_id?: string;
}

export interface ErrorMessage extends BaseMessage {
  type: "error";
  content: string;
}

export type Message = UserMessage | BotMessage | ToolMessage | ErrorMessage;

export interface ProjectState {
  id: string;
  name: string;
  dir: string;
  created_at: string;
  updated_at: string;
}

export interface PendingPermission {
  req_id: string;
  session_id?: string;
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
  session_id?: string;
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
  /** If this session is a subagent, its parent session ID */
  parent_session_id?: string;
  messages: Message[];
  phase: string;
  is_running: boolean;
  checkpoints: unknown[];
  tabs: Tab[];
  active_tab_id: string;
  pending_permissions: PendingPermission[];
  pending_ask_users: PendingAskUser[];
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
  /** 当前流式接收的工具调用名称，由 tool_call_delta 事件设置 */
  streaming_tool_name?: string;
  git_info?: GitInfo | null;
  goal?: { description: string; status: string } | null;
  todos?: { id: string; content: string; status: string }[];
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
export const sessionCursors = $state<Record<string, string>>({});
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

export const streamingMessages = $state<Record<string, Message[]>>({});

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

export function getDisplayMessages(session_id: string): Message[] {
  const session = getSession(session_id);
  if (!session) return [];
  const streamBuf = streamingMessages[session_id] ?? [];
  if (streamBuf.length === 0) return session.messages;

  // 基于 ID 去重：如果 streaming buffer 的第一条已存在于 session.messages
  // 中（后端已通过 loadSessionMessages 重新加载），直接跳过重复项。
  const seen = new Set(session.messages.map((m) => m.id));
  const deduped = streamBuf.filter((m) => !seen.has(m.id));
  return [...session.messages, ...deduped];
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
            is_running: false,
            checkpoints: [],
            tabs: [],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_users: [],
            queued_input: null,
            updated_at: s.created_at,
            permission_level: s.auto_approve_level,
            goal: null,
            todos: [],
          });
        } else {
          current.alias = s.title ?? current.alias;
          current.updated_at = s.created_at ?? current.updated_at;
          current.permission_level =
            s.auto_approve_level ?? current.permission_level;
          current.goal ??= null;
          current.todos ??= [];
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
            is_running: false,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_users: [],
            queued_input: null,
            updated_at: p.updated_at,
            is_pinned: true,
            goal: null,
            todos: [],
          };
          sessionState.sessions.push(session);
        } else {
          session.is_pinned = true;
          session.alias = p.title ?? session.alias ?? "Untitled";
          session.updated_at = p.updated_at ?? session.updated_at;
          session.project_id = p.project_id ?? session.project_id;
          session.goal ??= null;
          session.todos ??= [];
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
    api.unsubscribe(prevId).catch(() => {});
    const prevSession = getSession(prevId);
    if (prevSession) {
      prevSession.streaming_tool_name = undefined;
    }
  }
  if (id) {
    api.subscribe(id, null).catch(() => {});
    // 切入新 session 时也清除可能残留的 streaming_tool_name（如之前异常中断）
    const nextSession = getSession(id);
    if (nextSession) {
      nextSession.streaming_tool_name = undefined;
    }
  }
  sessionState.activeSessionId = id;
}

export function upsertSession(session: SessionState) {
  const idx = sessionState.sessions.findIndex((s) => s.id === session.id);
  if (idx >= 0) {
    sessionState.sessions[idx] = session;
  } else {
    sessionState.sessions.push(session);
  }
}

export async function loadSessionData(sessionId: string) {
  const info = await api.getSession(sessionId);
  const session: SessionState = {
    id: sessionId,
    project_path: info.working_dir || "",
    alias: info.title || undefined,
    parent_session_id: info.parent_id || undefined,
    messages: [],
    phase: info.phase,
    is_running: info.phase !== "idle" && info.phase !== "closed",
    checkpoints: [],
    tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
    active_tab_id: "chat",
    pending_permissions: [],
    pending_ask_users: [],
    queued_input: null,
    updated_at: new Date().toISOString(),
    permission_level: info.auto_approve_level || undefined,
    goal: null,
    todos: [],
  };
  upsertSession(session);
  const [msgs, goalResult, todosResult] = await Promise.all([
    api.getMessages(sessionId),
    api.getGoal(sessionId).catch(() => null),
    api.getTodos(sessionId).catch(() => ({ todos: [] })),
  ]);
  session.goal = goalResult;
  session.todos = todosResult.todos;
  loadSessionMessages(sessionId, msgs);
  return session;
}

export async function activateSession(sessionId: string) {
  if (!sessionId) return;
  const existing = inFlightActivations.get(sessionId);
  if (existing) return existing;

  setActiveSession(sessionId); // 立即切换，给用户即时反馈

  const promise = (async () => {
    try {
      if (!getSession(sessionId)) {
        await loadSessionData(sessionId);
      } else {
        // Session already exists but messages may be stale — reload them
        const msgs = await api.getMessages(sessionId);
        loadSessionMessages(sessionId, msgs);
      }
    } finally {
      inFlightActivations.delete(sessionId);
    }
  })();

  inFlightActivations.set(sessionId, promise);
  return promise;
}

export function syncSessionStatus(session_id: string, info: { phase: string }) {
  const session = getSession(session_id);
  if (!session) return;
  session.phase = info.phase;
  session.is_running = info.phase !== "idle" && info.phase !== "closed";
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

export function loadSessionMessages(
  session_id: string,
  messages: SessionMessage[],
) {
  const session = getSession(session_id);
  if (!session) return;

  const parsedMessages: Message[] = [];
  for (const m of messages) {
    switch (m.kind) {
      case "user": {
        parsedMessages.push({
          id: m.id,
          type: "user",
          content: m.content ?? [],
          created_at: m.created_at,
        });
        break;
      }
      case "assistant": {
        parsedMessages.push({
          id: m.id,
          type: "assistant",
          content: m.content ?? [],
          tool_calls: m.tool_calls?.map((tc) => ({
            id: tc.id,
            name: tc.name,
            arguments: tc.arguments ?? "",
          })),
          token_usage: m.token_usage
            ? {
                prompt_tokens: m.token_usage.prompt_tokens,
                completion_tokens: m.token_usage.completion_tokens,
                total_tokens: m.token_usage.total_tokens,
              }
            : undefined,
          created_at: m.created_at,
        });
        break;
      }
      case "tool": {
        parsedMessages.push({
          id: m.id,
          type: "tool",
          tool_call_id: m.tool_call_id,
          tool_name: m.name,
          status: "completed",
          arguments: m.args ?? "",
          result: m.result ?? [],
          created_at: m.created_at,
          subagent_session_id: m.meta?.subagent_session_id,
        });
        break;
      }
    }
  }

  // Find the latest token usage from assistant messages (aligns with TUI logic)
  let latestTokenUsage = session.token_usage;
  for (let i = parsedMessages.length - 1; i >= 0; i--) {
    const msg = parsedMessages[i];
    if (msg.type === "assistant" && msg.token_usage) {
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
  request?: { message_id: string; message_count: number };
  chunk?: { message_id: string; content: ChunkContent };
  tool_call_delta?: {
    message_id: string;
    tool_id: string;
    tool_name: string;
    arguments_delta: string;
  };
  completed?: { message_id: string };
  error?: { message_id: string; error: string };
  fallback?: { message_id: string; from: string; to: string };
  compacting?: { active: boolean };
  token_usage?: {
    message_id: string;
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
    context_window: number;
  };
}

interface ToolStart {
  message_id: string;
  tool_id: string;
  tool_name: string;
  arguments?: string;
}

interface ToolEnd {
  message_id: string;
  tool_id: string;
  tool_name: string;
  is_error: boolean;
  elapsed_ms: number;
  content_blocks?: TaggedContentBlock[];
}

interface ToolEvent {
  start?: ToolStart;
  end?: ToolEnd;
  metadata?: ToolMetadata;
}

interface ToolMetadata {
  message_id: string;
  tool_id: string;
  metadata: Record<string, string>;
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
  state_changed?: {
    state: "idle" | "streaming" | "executing_tool" | "compacting" | "closed";
  };
  error?: {
    phase: string;
    error: string;
    is_recoverable: boolean;
  };
  permission_request?: {
    req_id: string;
    session_id?: string;
    tool_name: string;
    tool_args?: string;
    tool_level?: string;
    reason?: string;
  };
  permission_ack?: {
    req_id: string;
  };
  ask_user_question?: {
    req_id: string;
    session_id?: string;
    questions: AskQuestion[];
  };
  ask_user_ack?: {
    req_id: string;
  };
  retrying?: {
    attempt: number;
    max_attempts: number;
    reason: string;
  };
  rewound?: Record<string, never>;
  goal_updated?: { description: string; status: string };
  goal_stopped?: Record<string, never>;
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
  | { tool: ToolEvent }
  | { user: UserEvent };

export function handleEvent(
  session_id: string,
  event_id: string | undefined,
  rawEvent: unknown,
) {
  const session = getSession(session_id);
  if (!session) return;

  const ev = rawEvent as KernelEvent;
  // 只有 model.tool_call_delta 表示正在 calling tool，其余事件都清除该状态
  const isToolCalling = "model" in ev && ev.model.tool_call_delta != null;
  if (!isToolCalling) {
    session.streaming_tool_name = undefined;
  }
  if ("model" in ev) {
    handleModelEvent(session, ev.model);
  } else if ("agent" in ev) {
    handleAgentEvent(session, ev.agent);
  } else if ("tool" in ev) {
    handleToolEvent(session, ev.tool);
  } else if ("user" in ev) {
    handleUserEvent(session, ev.user);
  }
}

function findMessageById(
  session: SessionState,
  message_id: string,
): Message | undefined {
  const allMessages = [
    ...session.messages,
    ...(streamingMessages[session.id] ?? []),
  ];
  for (let i = allMessages.length - 1; i >= 0; i--) {
    const msg = allMessages[i];
    if (msg.id === message_id) return msg;
  }
  return undefined;
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

    if (content?.text) {
      const text = content.text;
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      if (
        lastMsg &&
        lastMsg.type === "assistant" &&
        lastMsg.id === chunk.message_id
      ) {
        const lastBlock = lastMsg.content[lastMsg.content.length - 1];
        if (
          lastBlock &&
          lastBlock.type === "text" &&
          typeof lastBlock.text === "string"
        ) {
          lastBlock.text += text;
        } else {
          lastMsg.content.push({ type: "text", text });
        }
      } else {
        buf.push({
          id: chunk.message_id,
          type: "assistant",
          content: [{ type: "text", text }],
          created_at: new Date().toISOString(),
        });
      }
      streamingMessages[session.id] = buf;
      return true;
    } else if (content?.thinking) {
      const buf = streamingMessages[session.id] ?? [];
      const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
      const thinkingText = content.thinking.thinking ?? "";
      if (
        lastMsg &&
        lastMsg.type === "assistant" &&
        lastMsg.id === chunk.message_id
      ) {
        const lastBlock = lastMsg.content[lastMsg.content.length - 1];
        if (
          lastBlock &&
          lastBlock.type === "thinking" &&
          typeof lastBlock.thinking === "string"
        ) {
          lastBlock.thinking += thinkingText;
        } else {
          lastMsg.content.push({
            type: "thinking",
            thinking: thinkingText,
            signature: undefined,
          });
        }
      } else {
        buf.push({
          id: chunk.message_id,
          type: "assistant",
          content: [
            { type: "thinking", thinking: thinkingText, signature: undefined },
          ],
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
    if (delta.tool_name) {
      session.streaming_tool_name = delta.tool_name;
    }
    const buf = streamingMessages[session.id] ?? [];
    const lastMsg = buf.length > 0 ? buf[buf.length - 1] : null;
    let botMsg: BotMessage;
    if (
      !lastMsg ||
      lastMsg.type !== "assistant" ||
      lastMsg.id !== delta.message_id
    ) {
      botMsg = {
        id: delta.message_id,
        type: "assistant",
        content: [],
        created_at: new Date().toISOString(),
      };
      buf.push(botMsg);
    } else {
      botMsg = lastMsg;
    }
    if (!botMsg.tool_calls) botMsg.tool_calls = [];
    let toolCall = botMsg.tool_calls.find((t) => t.id === delta.tool_id);
    if (!toolCall) {
      toolCall = {
        id: delta.tool_id,
        name: delta.tool_name,
        arguments: "",
      };
      botMsg.tool_calls.push(toolCall);
    }
    if (delta.arguments_delta) {
      toolCall.arguments = (toolCall.arguments ?? "") + delta.arguments_delta;
    }
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.compacting) {
    const active = event.compacting.active;
    if (!active) {
      // Compaction finished — reload messages to reflect compacted history
      streamingMessages[session.id] = [];
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

function maybeRefreshTodos(session: SessionState, toolName: string) {
  if (toolName === "todo") {
    api.getTodos(session.id)
      .then((r) => { session.todos = r.todos; })
      .catch(() => {});
  }
}

function handleToolEvent(session: SessionState, event: ToolEvent): boolean {
  if (event.start) {
    const start = event.start;
    const msg = findMessageById(session, start.message_id);
    if (msg && msg.type === "tool") {
      msg.status = "running";
      if (start.arguments) msg.arguments = start.arguments;
      return true;
    }
    // Create ToolMessage in streaming buffer
    const buf = streamingMessages[session.id] ?? [];
    const toolMsg: ToolMessage = {
      id: start.message_id,
      type: "tool",
      tool_call_id: start.tool_id,
      tool_name: start.tool_name,
      status: "running",
      arguments: start.arguments ?? "",
      result: [],
      created_at: new Date().toISOString(),
    };
    buf.push(toolMsg);
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.metadata) {
    const md = event.metadata;
    const msg = findMessageById(session, md.message_id);
    if (msg && msg.type === "tool") {
      const sid = md.metadata["subagent_session_id"];
      if (sid) {
        msg.subagent_session_id = sid;
      }
      return true;
    }
    // Metadata may arrive before Start if subagent spawns very fast
    const buf = streamingMessages[session.id] ?? [];
    const toolMsg: ToolMessage = {
      id: md.message_id,
      type: "tool",
      tool_call_id: md.tool_id,
      tool_name: "agent",
      status: "running",
      arguments: "",
      result: [],
      created_at: new Date().toISOString(),
    };
    const sid = md.metadata["subagent_session_id"];
    if (sid) {
      toolMsg.subagent_session_id = sid;
    }
    buf.push(toolMsg);
    streamingMessages[session.id] = buf;
    return true;
  } else if (event.end) {
    const end = event.end;
    const msg = findMessageById(session, end.message_id);
    if (msg && msg.type === "tool") {
      msg.status = end.is_error ? "failed" : "completed";
      msg.elapsed_ms = end.elapsed_ms;
      msg.result = end.content_blocks ?? [];
      maybeRefreshTodos(session, end.tool_name);
      maybeRefreshGitInfo(session);
      return true;
    }
    // Start event may have been lost — reconstruct from End
    const buf = streamingMessages[session.id] ?? [];
    const toolMsg: ToolMessage = {
      id: end.message_id,
      type: "tool",
      tool_call_id: end.tool_id,
      tool_name: end.tool_name,
      status: end.is_error ? "failed" : "completed",
      arguments: "",
      result: end.content_blocks ?? [],
      created_at: new Date().toISOString(),
    };
    buf.push(toolMsg);
    streamingMessages[session.id] = buf;
    maybeRefreshTodos(session, end.tool_name);
    maybeRefreshGitInfo(session);
    return true;
  }
  return false;
}

function handleAgentEvent(session: SessionState, event: AgentEvent): boolean {
  if (event.state_changed) {
    session.phase = event.state_changed.state;
    session.is_running = session.phase !== "idle" && session.phase !== "closed";
    return true;
  }

  if (event.lifecycle) {
    const state = event.lifecycle.state;
    if (state === "running") {
      session.phase = "streaming";
      session.is_running = true;
      return true;
    } else if (typeof state === "object" && state.stopped) {
      session.phase = "idle";
      session.is_running = false;
      const buf = streamingMessages[session.id] ?? [];
      if (buf.length > 0) {
        // 基于 ID 去重：跳过已存在于 session.messages 中的 streaming buffer 项。
        const seen = new Set(session.messages.map((m) => m.id));
        const deduped = buf.filter((m) => !seen.has(m.id));
        if (deduped.length > 0) {
          session.messages = [...session.messages, ...deduped];
        }
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
            type: "error",
            content: msg,
            created_at: new Date().toISOString(),
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
            type: "error",
            content: errorMsg,
            created_at: new Date().toISOString(),
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
            type: "error",
            content: msg,
            created_at: new Date().toISOString(),
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
  } else if (event.error) {
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      const seen = new Set(session.messages.map((m) => m.id));
      const deduped = buf.filter((m) => !seen.has(m.id));
      if (deduped.length > 0) {
        session.messages = [...session.messages, ...deduped];
      }
      streamingMessages[session.id] = [];
    }
    const errorStr = event.error.error ?? "Unknown";
    const errorMsg = "Agent error: " + errorStr;
    session.messages = [
      ...session.messages,
      {
        id: crypto.randomUUID(),
        type: "error",
        content: errorMsg,
        created_at: new Date().toISOString(),
      },
    ];
    const level = event.error.is_recoverable ? "warn" : "error";
    showNotification(errorMsg, level, 5000);
    sendDesktopNotification("Yomi", errorMsg, session.id);
    return true;
  } else if (event.retrying) {
    const retry = event.retrying;
    const msg = `Agent retrying (${retry.attempt}/${retry.max_attempts})`;
    session.messages = [
      ...session.messages,
      {
        id: crypto.randomUUID(),
        type: "error",
        content: msg,
        created_at: new Date().toISOString(),
      },
    ];
    showNotification(msg, "warn", 3000);
    return true;
  } else if (event.permission_request) {
    const req = event.permission_request;
    session.pending_permissions.push({
      req_id: req.req_id,
      session_id: req.session_id,
      tool_name: req.tool_name,
      tool_args: req.tool_args ?? "",
      tool_level: req.tool_level ?? "safe",
      reason: req.reason ?? "",
    });
    showNotification(`${req.tool_name} needs approval`, "warn", 5000);
    return true;
  } else if (event.ask_user_question) {
    const req = event.ask_user_question;
    session.pending_ask_users.push({
      req_id: req.req_id,
      session_id: req.session_id,
      questions: req.questions,
    });
    showNotification("Agent has a question for you", "info", 5000);
    sendDesktopNotification("Yomi", "Agent has a question for you", session.id);
    return true;
  } else if (event.permission_ack) {
    const req_id = event.permission_ack.req_id;
    const idx = session.pending_permissions.findIndex(
      (p) => p.req_id === req_id,
    );
    if (idx >= 0) {
      session.pending_permissions = session.pending_permissions.toSpliced(
        idx,
        1,
      );
    }
    return true;
  } else if (event.ask_user_ack) {
    const req_id = event.ask_user_ack.req_id;
    const idx = session.pending_ask_users.findIndex((a) => a.req_id === req_id);
    if (idx >= 0) {
      session.pending_ask_users = session.pending_ask_users.toSpliced(idx, 1);
    }
    return true;
  } else if (event.rewound) {
    // Clear any streaming buffer since history changed
    streamingMessages[session.id] = [];
    session.phase = "idle";
    session.is_running = false;
    // Reload messages from backend instead of using event payload
    api
      .getMessages(session.id)
      .then((msgs) => loadSessionMessages(session.id, msgs))
      .catch((e: Error) =>
        console.error("Failed to reload messages after rewind:", e),
      );
    // Refresh checkpoints list after rewind
    refreshCheckpoints(session.id);
    showNotification("Session rewound", "info", 3000);
    return true;
  } else if (event.goal_updated) {
    session.goal = {
      description: event.goal_updated.description,
      status: event.goal_updated.status,
    };
    return true;
  } else if (event.goal_stopped) {
    session.goal = null;
    return true;
  }
  return false;
}

function handleUserEvent(session: SessionState, event: UserEvent): boolean {
  if (event.message) {
    const msg = event.message;
    session.messages.push({
      id: msg.message_id,
      type: "user",
      content: msg.content ?? [],
      created_at: new Date().toISOString(),
    });
    session.updated_at = new Date().toISOString();
    return true;
  }
  return false;
}
