import * as api from "./api";
import type { GitInfo } from "./api";
import type { TaggedContentBlock } from "./types";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";

// 非活跃 session 自动 unsubscribe 延迟（60 秒）
const INACTIVE_UNSUBSCRIBE_DELAY = 60_000;

const inFlightActivations = new Map<string, Promise<void>>();

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
  subagent_session_id?: string;
}

interface BaseMessage {
  id: string;
  created_at: string;
}

export interface UserMessage extends BaseMessage {
  type: "user";
  content: string;
  content_blocks?: TaggedContentBlock[];
}

export interface BotMessage extends BaseMessage {
  type: "assistant";
  content: string;
  thinking?: { content: string; elapsed_ms: number } | null;
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
  output: string;
  elapsed_ms?: number;
  subagent_session_id?: string;
}

export interface SystemMessage extends BaseMessage {
  type: "system";
  content: string;
}

export interface ErrorMessage extends BaseMessage {
  type: "error";
  content: string;
}

export type Message =
  | UserMessage
  | BotMessage
  | ToolMessage
  | SystemMessage
  | ErrorMessage;

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
            is_running: false,
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
    unread: 0,
    checkpoints: [],
    tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
    active_tab_id: "chat",
    pending_permissions: [],
    pending_ask_user: null,
    queued_input: null,
    updated_at: new Date().toISOString(),
    permission_level: info.auto_approve_level || undefined,
  };
  upsertSession(session);
  const msgs = await api.getMessages(sessionId);
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
      await api.subscribe(sessionId);
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
): "user" | "tool" | "system" | "assistant" | "error" | "internal" {
  if (role === "User" || role === "user") return "user";
  if (role === "tool" || role === "Tool") return "tool";
  if (role === "system" || role === "System") return "system";
  if (role === "internal" || role === "Internal") return "internal";
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
  _meta?: Record<string, string>;
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

  const toolOutputs: Record<string, string> = {};
  const toolOutputByName: Record<string, string> = {};
  const toolMeta: Record<string, Record<string, string>> = {};
  const toolCallDecls: Record<string, { name: string; arguments: string }> = {};

  for (const m of rawMessages as RawMessage[]) {
    const role = normalizeRole(m.role);
    if (role === "internal") {
      if (m._meta && m.tool_call_id) {
        toolMeta[m.tool_call_id] = m._meta;
      }
      continue;
    }

    // Collect tool call declarations from assistant messages
    if (role === "assistant" && Array.isArray(m.tool_calls)) {
      for (const tc of m.tool_calls) {
        const id = tc.id || "";
        if (!id) continue;
        const name = tc.name || tc.tool_name || "";
        let args = "";
        if (tc.arguments) {
          args =
            typeof tc.arguments === "string"
              ? tc.arguments
              : JSON.stringify(tc.arguments);
        }
        toolCallDecls[id] = { name, arguments: args };
        const cleanId = id.replace(/^functions\./, "");
        if (cleanId !== id) {
          toolCallDecls[cleanId] = { name, arguments: args };
        }
      }
      continue;
    }

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
      if (m._meta) {
        toolMeta[m.tool_call_id] = m._meta;
        if (cleanId !== m.tool_call_id) {
          toolMeta[cleanId] = m._meta;
        }
      }
    }
    if (m.id && typeof m.id === "string") {
      toolOutputs[m.id] = output;
      if (m._meta) {
        toolMeta[m.id] = m._meta;
      }
    }
  }

  // Second pass: build all messages as separate types
  const parsedMessages: Message[] = [];
  for (const m of rawMessages as RawMessage[]) {
    const role = normalizeRole(m.role);

    if (role === "internal") {
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
        if (m.content.some((b: TaggedContentBlock) => b.type !== "text")) {
          blocks = m.content;
        }
      } else if (typeof m.content === "string") {
        textContent = m.content;
      }
      parsedMessages.push({
        id: extractId(m.id),
        type: "user",
        content: textContent,
        content_blocks: blocks,
        created_at: m.created_at || new Date().toISOString(),
      });
    } else if (role === "tool") {
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
      const tci = m.tool_call_id || "";
      const decl = toolCallDecls[tci] ||
        toolCallDecls[tci.replace(/^functions\./, "")] || {
          name: "",
          arguments: "",
        };
      parsedMessages.push({
        id: extractId(m.id),
        type: "tool",
        tool_call_id: tci,
        tool_name: decl.name,
        // Messages loaded from DB are always completed (running only exists in streaming)
        status: "completed",
        arguments: decl.arguments,
        output,
        created_at: m.created_at || new Date().toISOString(),
        subagent_session_id: toolMeta[tci]?.subagent_session_id,
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
        type: role as "system" | "error",
        content: text,
        created_at: m.created_at || new Date().toISOString(),
      });
    } else if (role === "assistant") {
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

      // Extract tool_calls declarations from the message
      const tool_calls: { id: string; name: string; arguments: string }[] = [];
      if (Array.isArray(m.tool_calls)) {
        for (const tc of m.tool_calls) {
          const id = tc.id || "";
          const name = tc.name || tc.tool_name || "";
          let args = "";
          if (tc.arguments) {
            args =
              typeof tc.arguments === "string"
                ? tc.arguments
                : JSON.stringify(tc.arguments);
          }
          tool_calls.push({ id, name, arguments: args });
        }
      }

      parsedMessages.push({
        id: extractId(m.id),
        type: "assistant",
        content: text,
        thinking,
        tool_calls: tool_calls.length > 0 ? tool_calls : undefined,
        token_usage: m.token_usage
          ? {
              prompt_tokens: m.token_usage.prompt_tokens,
              completion_tokens: m.token_usage.completion_tokens,
              total_tokens: m.token_usage.total_tokens,
            }
          : undefined,
        created_at: m.created_at || new Date().toISOString(),
      });
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
    tool_name: string;
    tool_args?: string;
    tool_level?: string;
    reason?: string;
  };
  ask_user_question?: {
    req_id: string;

    questions: AskQuestion[];
  };
  retrying?: {
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

/** Schedule unsubscribe if the session is not currently active. */
function scheduleUnsubscribeIfInactive(session: SessionState) {
  if (session.id !== sessionState.activeSessionId) {
    scheduleUnsubscribe(session.id);
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
        lastMsg.content += text;
      } else {
        buf.push({
          id: chunk.message_id,
          type: "assistant",
          content: text,
          thinking: null,
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
        lastMsg.type === "assistant" &&
        lastMsg.id === chunk.message_id
      ) {
        if (!lastMsg.thinking)
          lastMsg.thinking = { content: "", elapsed_ms: 0 };
        lastMsg.thinking.content += content.thinking.thinking ?? "";
      } else {
        buf.push({
          id: chunk.message_id,
          type: "assistant",
          content: "",
          thinking: { content: content.thinking.thinking ?? "", elapsed_ms: 0 },
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
        content: "",
        thinking: null,
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
    // Streaming chunks finished — merge buffer with dedup.
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      const seen = new Set(session.messages.map((m) => m.id));
      const deduped = buf.filter((m) => !seen.has(m.id));
      if (deduped.length > 0) {
        session.messages = [...session.messages, ...deduped];
      }
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
      output: "",
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
      tool_name: "subagent",
      status: "running",
      arguments: "",
      output: "",
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
      msg.output =
        end.content_blocks
          ?.map((b: TaggedContentBlock) => {
            if (typeof b === "string") return b;
            return b.type === "text" && b.text ? b.text : "";
          })
          .join("") ?? "";
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
      output:
        end.content_blocks
          ?.map((b: TaggedContentBlock) => {
            if (typeof b === "string") return b;
            return b.type === "text" && b.text ? b.text : "";
          })
          .join("") ?? "",
      created_at: new Date().toISOString(),
    };
    buf.push(toolMsg);
    streamingMessages[session.id] = buf;
    maybeRefreshGitInfo(session);
    return true;
  }
  return false;
}

function handleAgentEvent(session: SessionState, event: AgentEvent): boolean {
  if (event.state_changed) {
    session.phase = event.state_changed.state;
    session.is_running = session.phase !== "idle" && session.phase !== "closed";
    if (session.is_running) {
      cancelPendingUnsubscribe(session.id);
    } else {
      scheduleUnsubscribeIfInactive(session);
    }
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
      scheduleUnsubscribeIfInactive(session);
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
    scheduleUnsubscribeIfInactive(session);
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
      type: "user",
      content,
      content_blocks: hasNonText ? msg.content : undefined,
      created_at: new Date().toISOString(),
    });
    session.updated_at = new Date().toISOString();
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
    session.phase = "closed";
    session.is_running = false;
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
    session.is_running = false;
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
