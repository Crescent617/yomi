import type {
  Checkpoint,
  GitInfo,
  RunningSessionInfo,
  SubagentInfo,
} from "./api";
import {
  getSession as fetchSession,
  listRunningSessions as fetchRunningSessions,
  listSubagents as fetchSubagents,
} from "./api";
import type { TaggedContentBlock } from "./types";
import { listen } from "@tauri-apps/api/event";
import { pushToast } from "./toast.svelte";
import { guiPreferences } from "./settings.svelte";
import {
  addSessionCompletion,
  didSessionComplete,
  seedRunningSessionStatuses,
  type AttentionItem,
} from "./attention-box";
import {
  captureSessionPhaseRevisions,
  isActiveSessionPhase,
  reconcileRunningSessionPhases,
  setSessionPhase,
} from "./session-phase";
import {
  clearQueuedMessage,
  flushQueuedMessage,
} from "./queued-messages.svelte";

// ── Kernel notification listener ─────────────────────────────────────────

const subagentRefreshes = new Map<string, Promise<void>>();
const dirtySubagentParents = new Set<string>();
let runningSessionsRefresh: Promise<void> | null = null;
let runningSessionsDirty = false;
let attentionItemSequence = 0;
const lastKnownSessionStatus = new Map<string, string>();

export const runningSessions = $state<RunningSessionInfo[]>([]);

/** Running sessions in an active phase (streaming / executing_tool / compacting). */
export const streamingSessions = $derived(
  runningSessions.filter((session) => isActiveSessionPhase(session.phase)),
);

export function refreshRunningSessions(): Promise<void> {
  runningSessionsDirty = true;
  if (runningSessionsRefresh) return runningSessionsRefresh;

  runningSessionsRefresh = (async () => {
    try {
      while (runningSessionsDirty) {
        runningSessionsDirty = false;
        const revisionsAtRequest = captureSessionPhaseRevisions(
          sessionState.sessions,
        );
        const sessions = await fetchRunningSessions();
        const revisionsChanged = sessionState.sessions.some(
          (session) =>
            session.phase_revision !== revisionsAtRequest.get(session.id),
        );
        if (runningSessionsDirty || revisionsChanged) {
          runningSessionsDirty = true;
          continue;
        }
        runningSessions.splice(0, runningSessions.length, ...sessions);
        reconcileRunningSessionPhases(
          sessionState.sessions,
          sessions,
          revisionsAtRequest,
        );
        seedRunningSessionStatuses(
          lastKnownSessionStatus,
          sessions.map((session) => session.id),
        );
      }
    } catch {
      // Keep the last authoritative snapshot; the next status change retries.
    } finally {
      runningSessionsRefresh = null;
    }
  })();
  return runningSessionsRefresh;
}

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

async function recordSessionCompletion(
  sessionId: string,
  completedAt: string,
): Promise<void> {
  const loadedSession = getSession(sessionId);
  let title = loadedSession?.alias;
  let projectId = loadedSession?.project_id;
  if (!loadedSession) {
    try {
      const info = await fetchSession(sessionId);
      title = info.title ?? undefined;
      projectId = info.project_id ?? undefined;
    } catch {
      // The session may have been deleted before its completion event arrived.
      return;
    }
  }

  const next = addSessionCompletion(attentionItems, {
    id: `${sessionId}:${completedAt}:${attentionItemSequence++}`,
    sessionId,
    title: title || "Untitled session",
    projectId: projectId ?? null,
    completedAt,
    read: sessionState.activeSessionId === sessionId,
  });
  attentionItems.splice(0, attentionItems.length, ...next);
}

export async function startNotificationListener(): Promise<() => void> {
  const unlisten = await listen(
    "kernel:noti",
    (e: {
      payload: {
        state_changed?: { session_id: string; status: string };
        title_updated?: { session_id: string; title: string };
        connection_lost?: { session_id: string };
        background_tasks_changed?: {
          session_id: string;
          kind: "subagent" | "shell";
        };
        agent_activity?: {
          session_id: string;
          activity: { kind: string; reason?: StopReason };
        };
      };
    }) => {
      const payload = e.payload;
      if (payload.state_changed) {
        const { session_id, status } = payload.state_changed;
        const previousStatus = lastKnownSessionStatus.get(session_id);
        lastKnownSessionStatus.set(session_id, status);
        const session = getSession(session_id);
        const completed = didSessionComplete(previousStatus, status);
        if (!session_id.startsWith("sub_") && completed) {
          const completedAt = new Date().toISOString();
          void recordSessionCompletion(session_id, completedAt);
        }
        if (session) {
          setSessionPhase(session, status);
        }
        if (
          status === "idle" &&
          !session_id.startsWith("sub_") &&
          sessionState.activeSessionId !== session_id
        ) {
          unreadSessions[session_id] = true;
        }
        if (session_id.startsWith("sub_")) refreshSubagentParent(session_id);
        void refreshRunningSessions();
      }
      if (payload.agent_activity) {
        const { session_id, activity } = payload.agent_activity;
        // Auto-send the queued message only after a successful run; on
        // failure/cancel it stays queued for the user to review.
        if (
          activity.kind === "stopped" &&
          activity.reason &&
          "completed" in activity.reason
        ) {
          void flushQueuedMessage(session_id).then((ok) => {
            if (!ok) showNotification("Failed to send queued message", "error");
          });
        }
      }
      if (payload.title_updated) {
        const { session_id, title } = payload.title_updated;
        if (session_id.startsWith("sub_")) refreshSubagentParent(session_id);
        const session = getSession(session_id);
        if (session) session.alias = title;
      }
      if (payload.background_tasks_changed?.kind === "shell") {
        void refreshRunningSessions();
      }
      if (payload.connection_lost) {
        lastKnownSessionStatus.clear();
        runningSessions.splice(0, runningSessions.length);
        showNotification("Connection lost", "warning");
      }
    },
  );
  for (const session of sessionState.sessions) {
    if (!lastKnownSessionStatus.has(session.id)) {
      lastKnownSessionStatus.set(session.id, session.phase);
    }
  }
  await refreshRunningSessions();
  return unlisten;
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
  /** Image URLs from tool output blocks (e.g. read tool on an image file). */
  images?: string[];
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
  /** Populated only by live streaming events (tool_call_delta); the
   *  list_messages API does not carry tool calls for assistant messages. */
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

export interface SessionState {
  id: string;
  project_path: string;
  project_id?: string;
  alias?: string;
  parent_session_id?: string;
  messages: Message[];
  message_rewrite_revision: number;
  /** True once the initial message history has been fetched from the backend. */
  messages_loaded: boolean;
  phase: string;
  phase_revision: number;
  checkpoints: Checkpoint[];
  tabs: Tab[];
  active_tab_id: string;
  pending_permissions: PendingPermission[];
  pending_ask_users: PendingAskUser[];
  updated_at: string;
  permission_level?: string;
  is_pinned?: boolean;
  model_key?: string;
  token_usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  /** Run-cumulative streamed output for the inline status line: in-flight
   *  response bytes (reset per request — a retried attempt is discarded)
   *  plus real completion tokens folded at each response end (`pending`
   *  holds the latest usage report until then). Cleared when the run stops. */
  out_stream?: { text: number; json: number; run: number; pending?: number };
  streaming_tool_name?: string;
  git_info?: GitInfo | null;
  git_refresh_revision?: number;
  goal?: { description: string; status: string } | null;
  todos?: { id: number; content: string; status: string }[];
  subagents: SubagentInfo[];
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
  end?: { message_id: string };
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

export type StopReason =
  | { cancelled: { operation?: string } }
  | { failed: { error: string } }
  | { max_iterations: { reached: number } }
  | { completed: { finish_reason?: string | null } };

export interface AgentLifecycleStopped {
  state: {
    stopped: {
      reason: StopReason;
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

export type ActivePanel =
  | "chat"
  | "usage"
  | "debug"
  | "config"
  | "automation"
  | "favorites";

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
  config_restart_required: false,
  config_applied: false,
});

export function requestActivePanel(panel: ActivePanel): boolean {
  if (panel === appState.activePanel) return true;
  if (
    appState.activePanel === "config" &&
    appState.config_dirty &&
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

export const attentionItems = $state<AttentionItem[]>([]);
export const unreadSessions = $state<Record<string, boolean>>({});

export function markAttentionItemRead(id: string): void {
  const item = attentionItems.find((entry) => entry.id === id);
  if (item) item.read = true;
}

export function removeProjectAttentionItems(
  projectId: string,
  sessionIds: Set<string>,
): void {
  const kept = attentionItems.filter(
    (item) => item.projectId !== projectId && !sessionIds.has(item.sessionId),
  );
  attentionItems.splice(0, attentionItems.length, ...kept);
}

export function removeSessionAttentionItems(sessionIds: Set<string>): void {
  const kept = attentionItems.filter(
    (item) => !sessionIds.has(item.sessionId),
  );
  attentionItems.splice(0, attentionItems.length, ...kept);
}

export function markAllAttentionItemsRead(): void {
  for (const item of attentionItems) item.read = true;
}

export const pinnedSessionMeta = $state(
  {} as Record<string, { pinned_at: string }>,
);

export const streamingMessages = $state<Record<string, Message[]>>({});

/** Per-session composer drafts — survives tab/session switches. */
export const inputDrafts = $state<Record<string, string>>({});

/**
 * Drop all ephemeral per-session UI state. Call when a session is deleted
 * so stale entries never linger or resurface for a reused id.
 */
export function purgeSessionLocalState(sessionId: string): void {
  clearQueuedMessage(sessionId);
  delete inputDrafts[sessionId];
  delete unreadSessions[sessionId];
  delete pinnedSessionMeta[sessionId];
}

// ── Scroll-to-message requests (e.g. favorites → chat) ─────────────────

export const scrollToMessageRequest = $state<{
  messageId: string | null;
  at: number;
}>({ messageId: null, at: 0 });

export function requestScrollToMessage(messageId: string): void {
  scrollToMessageRequest.messageId = messageId;
  scrollToMessageRequest.at = Date.now();
}

export function clearScrollToMessageRequest(): void {
  scrollToMessageRequest.messageId = null;
}

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
