import type { Checkpoint, GitInfo, SubagentInfo } from "./api";
import {
  getSession as fetchSession,
  listSubagents as fetchSubagents,
} from "./api";
import type { TaggedContentBlock } from "./types";
import { listen } from "@tauri-apps/api/event";
import { pushToast } from "./toast.svelte";
import { guiPreferences } from "./settings.svelte";

// ── Kernel notification listener ─────────────────────────────────────────

const subagentRefreshes = new Map<string, Promise<void>>();
const dirtySubagentParents = new Set<string>();

export function refreshSubagents(parent_session_id: string): Promise<void> {
  dirtySubagentParents.add(parent_session_id);
  const existing = subagentRefreshes.get(parent_session_id);
  if (existing) return existing;

  const refresh = (async () => {
    try {
      while (dirtySubagentParents.delete(parent_session_id)) {
        const subagents = await fetchSubagents(parent_session_id);
        const parent = getSession(parent_session_id);
        if (parent) parent.subagents = subagents;
      }
    } catch {
      // Keep the current snapshot; activation or a later notification retries.
    } finally {
      subagentRefreshes.delete(parent_session_id);
    }
  })();
  subagentRefreshes.set(parent_session_id, refresh);
  return refresh;
}

function refreshSubagentParent(session_id: string) {
  for (const parent of sessionState.sessions) {
    if (parent.subagents.some((item) => item.id === session_id)) {
      void refreshSubagents(parent.id);
      return;
    }
  }
  if (!session_id.startsWith("sub_")) return;

  void fetchSession(session_id)
    .then((session) => {
      if (session.parent_id) return refreshSubagents(session.parent_id);
    })
    .catch(() => {
      // The subagent may not be persisted yet; a later notification retries.
    });
}

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
        if (session) {
          session.phase = status;
          session.is_running = status !== "idle" && status !== "closed";
        }
        if (session_id.startsWith("sub_")) refreshSubagentParent(session_id);
      }
      if (payload.title_updated) {
        const { session_id, title } = payload.title_updated;
        if (session_id.startsWith("sub_")) refreshSubagentParent(session_id);
        const session = getSession(session_id);
        if (session) session.alias = title;
      }
      if (payload.connection_lost) {
        showNotification("Connection lost", "warning");
      }
    },
  );
}

// ── Types ────────────────────────────────────────────────────────────────

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

export interface BaseMessage {
  id: string;
  created_at: string;
}

export interface UserMessage extends BaseMessage {
  type: "user";
  content: TaggedContentBlock[];
}

export interface SteerMessage extends BaseMessage {
  type: "steer";
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

export type Message =
  | UserMessage
  | SteerMessage
  | BotMessage
  | ToolMessage
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
  parent_session_id?: string;
  messages: Message[];
  message_rewrite_revision: number;
  phase: string;
  is_running: boolean;
  checkpoints: Checkpoint[];
  tabs: Tab[];
  active_tab_id: string;
  pending_permissions: PendingPermission[];
  pending_ask_users: PendingAskUser[];
  queued_input: QueuedInput | null;
  updated_at: string;
  permission_level?: string;
  is_pinned?: boolean;
  model_key?: string;
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  streaming_tool_name?: string;
  git_info?: GitInfo | null;
  git_refresh_revision?: number;
  goal?: { description: string; status: string } | null;
  todos?: { id: string; content: string; status: string }[];
  subagents: SubagentInfo[];
  browserUrl?: string;
}

// ── Kernel event types (deserialized from Rust Event enum) ───────────────

export interface ChunkContent {
  text?: string;
  thinking?: { thinking?: string; signature?: string };
  redacted_thinking?: null;
}

export interface ModelChunk {
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

export interface ToolStart {
  message_id: string;
  tool_id: string;
  tool_name: string;
  arguments?: string;
}

export interface ToolEnd {
  message_id: string;
  tool_id: string;
  tool_name: string;
  is_error: boolean;
  elapsed_ms: number;
  content_blocks?: TaggedContentBlock[];
}

export interface ToolEvent {
  start?: ToolStart;
  end?: ToolEnd;
  metadata?: ToolMetadata;
}

export interface ToolMetadata {
  message_id: string;
  tool_id: string;
  metadata: Record<string, string>;
}

export interface AgentLifecycleStopped {
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

export interface AgentLifecycleRunning {
  state: "running";
}

export type AgentLifecycle = AgentLifecycleRunning | AgentLifecycleStopped;

export interface AgentEvent {
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
  message_replaced?: { session_id: string };
  goal_updated?: { description: string; status: string };
  goal_stopped?: null;
}

export interface UserEvent {
  message?: {
    message_id: string;
    content: TaggedContentBlock[];
  };
  steer?: {
    message_id: string;
    content: TaggedContentBlock[];
  };
}

export type KernelEvent =
  | { model: ModelChunk }
  | { agent: AgentEvent }
  | { tool: ToolEvent }
  | { user: UserEvent };

// ── Core state ───────────────────────────────────────────────────────────

export type ActivePanel = "chat" | "usage" | "config" | "automation";

export const appState = $state({
  connectionStatus: "disconnected" as
    | "connected"
    | "disconnected"
    | "connecting",
  currentTheme: "system" as "light" | "dark" | "system",
  sidebarCollapsed: false,
  rightPanelCollapsed: true,
  activePanel: "chat" as ActivePanel,
  config_dirty: false,
  app_config_dirty: false,
  config_restart_required: false,
  config_applied: false,
});

export function requestActivePanel(panel: ActivePanel): boolean {
  if (panel === appState.activePanel) return true;
  if (
    appState.activePanel === "config" &&
    (appState.config_dirty || appState.app_config_dirty) &&
    typeof window !== "undefined" &&
    !window.confirm("You have unsaved config changes. Leave without saving?")
  ) {
    return false;
  }
  appState.activePanel = panel;
  return true;
}

export const projectState = $state({
  projects: [] as ProjectState[],
  activeProjectId: null as string | null,
});

export const sessionCursors = $state<Record<string, string>>({});

export const sessionState = $state({
  sessions: [] as SessionState[],
  activeSessionId: null as string | null,
});

export const pinnedSessionMeta = $state(
  {} as Record<string, { pinned_at: string }>,
);

export const streamingMessages = $state<Record<string, Message[]>>({});

// ── Notification helper ──────────────────────────────────────────────────

export function showNotification(
  text: string,
  level: "info" | "success" | "warning" | "error" = "info",
) {
  if (!guiPreferences.notifications.enabled) return;
  pushToast(text, level);
}

// ── Basic session accessors ────────────────────────────────────────────

export function getSession(session_id: string): SessionState | undefined {
  return sessionState.sessions.find((s) => s.id === session_id);
}

export function getActiveSession(): SessionState | null {
  return sessionState.activeSessionId
    ? (getSession(sessionState.activeSessionId) ?? null)
    : null;
}
