import * as api from "./api";
import type { SessionMessage } from "./api";
import type { TaggedContentBlock } from "./types";
import {
  getSession,
  pinnedSessionMeta,
  refreshSubagents,
  requestActivePanel,
  sessionState,
  showNotification,
  streamingMessages,
  type Message,
  type SessionState,
} from "./state.svelte";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";

const inFlightActivations = new Map<string, Promise<void>>();

// ── Helpers: content block utils ───────────────────────────────────────────

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

// ── Browser helpers ──────────────────────────────────────────────────────

export function syncSessionStatus(session_id: string, info: { phase: string }) {
  const session = getSession(session_id);
  if (!session) return;
  session.phase = info.phase;
  session.is_running = info.phase !== "idle" && info.phase !== "closed";
}

// ── Notifications ────────────────────────────────────────────────────────

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
          if (!requestActivePanel("chat")) return;
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

// ── Session lifecycle ──────────────────────────────────────────────────

/** Build a SessionState with sane defaults; override any field via `partial`. */
export function createSessionState(
  partial: Partial<SessionState> & { id: string },
): SessionState {
  return {
    project_path: "",
    messages: [],
    message_rewrite_revision: 0,
    phase: "idle",
    is_running: false,
    checkpoints: [],
    tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
    active_tab_id: "chat",
    pending_permissions: [],
    pending_ask_users: [],
    queued_input: null,
    updated_at: new Date().toISOString(),
    goal: null,
    todos: [],
    subagents: [],
    ...partial,
  };
}

export function replaceSessionMessages(
  session: SessionState,
  messages: Message[],
): void {
  session.messages = messages;
  session.message_rewrite_revision += 1;
}

export function appendSessionMessages(
  session: SessionState,
  messages: Message[],
): void {
  if (messages.length === 0) return;
  session.messages.push(...messages);
}

export function setActiveSession(id: string | null) {
  const prevId = sessionState.activeSessionId;
  if (id === prevId) return;
  if (prevId && id !== prevId) {
    api.unsubscribe(prevId).catch(() => {});
    const prevSession = getSession(prevId);
    if (prevSession) {
      prevSession.streaming_tool_name = undefined;
    }
  }
  if (id) {
    streamingMessages[id] = [];
    api.subscribe(id, null).catch(() => {});
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

async function hydrateSession(sessionId: string, session: SessionState) {
  const [info, msgs, goal, todos] = await Promise.all([
    api.getSession(sessionId),
    api.getMessages(sessionId),
    api.getGoal(sessionId).catch(() => null),
    api.getTodos(sessionId).catch(() => ({ todos: [] })),
    refreshSubagents(sessionId),
  ]);

  session.project_path = info.working_dir || session.project_path;
  session.project_id = info.project_id ?? session.project_id;
  session.alias = info.title ?? session.alias;
  session.parent_session_id = info.parent_id ?? undefined;
  session.permission_level =
    info.auto_approve_level ?? session.permission_level;
  session.model_key = info.model_key ?? session.model_key;
  session.updated_at = info.updated_at;
  session.goal = goal;
  session.todos = todos.todos;
  syncSessionStatus(sessionId, info);
  loadSessionMessages(sessionId, msgs);
}

export async function loadSessionData(sessionId: string) {
  let session = getSession(sessionId);
  if (!session) {
    session = createSessionState({ id: sessionId });
    upsertSession(session);
  }
  await hydrateSession(sessionId, session);
  return session;
}

export async function activateSession(sessionId: string) {
  if (!sessionId) return;
  const existing = inFlightActivations.get(sessionId);
  if (existing) return existing;

  const previousId = sessionState.activeSessionId;
  const createdPlaceholder = !getSession(sessionId);
  if (createdPlaceholder) {
    upsertSession(createSessionState({ id: sessionId }));
  }
  setActiveSession(sessionId);

  const promise = (async () => {
    try {
      await loadSessionData(sessionId);
    } catch (error) {
      if (createdPlaceholder) {
        sessionState.sessions = sessionState.sessions.filter(
          (session) => session.id !== sessionId,
        );
      }
      if (sessionState.activeSessionId === sessionId) {
        const rollbackId =
          previousId && getSession(previousId) ? previousId : null;
        setActiveSession(rollbackId);
      }
      showNotification(
        `Failed to open session: ${api.errorMessage(error)}`,
        "error",
      );
      throw error;
    } finally {
      inFlightActivations.delete(sessionId);
    }
  })();

  inFlightActivations.set(sessionId, promise);
  return promise;
}

// ── Data refresh ───────────────────────────────────────────────────────────

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
          sessionState.sessions.push(
            createSessionState({
              id: s.id,
              project_path: s.project_path,
              project_id: s.project_id,
              alias: s.title,
              tabs: [],
              updated_at: s.created_at,
              permission_level: s.auto_approve_level,
              model_key: s.model_key,
            }),
          );
        } else {
          current.alias = s.title ?? current.alias;
          current.updated_at = s.created_at ?? current.updated_at;
          current.permission_level =
            s.auto_approve_level ?? current.permission_level;
          current.model_key = s.model_key ?? current.model_key;
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
      for (const s of sessionState.sessions) {
        s.is_pinned = false;
      }
      for (const key of Object.keys(pinnedSessionMeta)) {
        delete pinnedSessionMeta[key];
      }

      for (const p of pinned) {
        pinnedSessionMeta[p.session_id] = {
          pinned_at: p.pinned_at,
        };

        let session = getSession(p.session_id);
        if (!session) {
          session = createSessionState({
            id: p.session_id,
            project_id: p.project_id,
            alias: p.title ?? "Untitled",
            updated_at: p.updated_at,
            is_pinned: true,
          });
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

// ── Tabs ─────────────────────────────────────────────────────────────────

export function openFileTab(
  session: SessionState,
  entry: {
    name: string;
    path: string;
    is_directory: boolean;
    is_file: boolean;
  },
  type: "preview" | "edit",
) {
  const existing = session.tabs.find(
    (t) => t.type === type && t.entry?.path === entry.path,
  );
  if (existing) {
    session.active_tab_id = existing.id;
    return;
  }
  const newTab = {
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

// ── Message loading ──────────────────────────────────────────────────────

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
      case "steer": {
        parsedMessages.push({
          id: m.id,
          type: "steer",
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

  let latestTokenUsage = session.token_usage;
  for (let i = parsedMessages.length - 1; i >= 0; i--) {
    const msg = parsedMessages[i];
    if (msg.type === "assistant" && msg.token_usage) {
      latestTokenUsage = msg.token_usage;
      break;
    }
  }

  replaceSessionMessages(session, parsedMessages);
  session.token_usage = latestTokenUsage;
}
