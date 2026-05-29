export interface Tab {
  id: string;
  type: "chat" | "preview" | "edit";
  label: string;
  entry?: { name: string; path: string; isDirectory: boolean };
  pinned?: boolean;
}

export interface ToolCall {
  id: string;
  toolName: string;
  status: "running" | "completed" | "failed" | "cancelled";
  arguments?: string;
  parsedArgs?: Record<string, any>;
  output?: string;
  error?: string;
  progress?: string;
  tokens?: number;
  elapsedMs?: number;
  folded?: boolean;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  thinking?: { content: string; elapsedMs: number } | null;
  tools?: ToolCall[];
}

export interface SessionState {
  id: string;
  projectPath: string;
  alias?: string;
  messages: ChatMessage[];
  streaming: boolean;
  unread: number;
  checkpoints: unknown[];
  tabs: Tab[];
  activeTabId: string;
}

export const appState = $state({
  connectionStatus: "disconnected" as "connected" | "disconnected" | "connecting",
  currentTheme: "system" as "light" | "dark" | "system",
  sidebarCollapsed: false,
});

export const sessionState = $state({
  sessions: [] as SessionState[],
  activeSessionId: null as string | null,
});

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
  upsertSession({
    ...session,
    messages: [
      ...session.messages,
      { id: crypto.randomUUID(), role: "user", content: text },
    ],
  });
}

export function openFileTab(
  session: SessionState,
  entry: any,
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

function extractId(raw: any): string {
  if (Array.isArray(raw) && raw.length > 0) raw = raw[0];
  else if (typeof raw === "object" && raw !== null) raw = raw["0"] ?? raw[0] ?? null;
  return typeof raw === "string" && raw.length > 0 ? raw : crypto.randomUUID();
}

export function loadSessionMessages(sessionId: string, rawMessages: any[]) {
  const session = getSession(sessionId);
  if (!session) return;
  upsertSession({
    ...session,
    messages: rawMessages.map((m) => {
      const role = m.role === "User" || m.role === "user" ? "user" : "assistant";
      let content = "";
      if (Array.isArray(m.content)) {
        content = m.content
          .map((block: any) => {
            if (typeof block === "string") return block;
            if (block.Text) return block.Text;
            if (block.text) return block.text;
            return "";
          })
          .join("");
      } else if (typeof m.content === "string") {
        content = m.content;
      }
      return {
        id: extractId(m.id),
        role,
        content,
        thinking: null,
        tools: [] as ToolCall[],
      };
    }),
  });
}

// Handle raw kernel events (serialized Event enum)
export function handleEvent(sessionId: string, rawEvent: any) {
  const session = getSession(sessionId);
  if (!session) return;

  let next = { ...session };
  let changed = false;

  if (rawEvent.Model) {
    changed = handleModelEvent(next, rawEvent.Model) || changed;
  } else if (rawEvent.Agent) {
    changed = handleAgentEvent(next, rawEvent.Agent) || changed;
  } else if (rawEvent.System) {
    changed = handleSystemEvent(next, rawEvent.System) || changed;
  } else if (rawEvent.Tool) {
    changed = handleToolEvent(next, rawEvent.Tool) || changed;
  }

  if (changed) {
    upsertSession(next);
  }

  if (sessionState.activeSessionId !== sessionId) {
    next.unread++;
    upsertSession(next);
  }
}

function handleModelEvent(session: SessionState, event: any): boolean {
  if (event.Chunk) {
    const chunk = event.Chunk;
    const content = chunk.content;
    if (content?.Text) {
      const text = content.Text;
      const lastMsg = session.messages[session.messages.length - 1];
      if (lastMsg && lastMsg.role === "assistant") {
        lastMsg.content += text;
        return true;
      } else {
        session.messages = [
          ...session.messages,
          { id: extractId(chunk.message_id), role: "assistant", content: text, tools: [] },
        ];
        return true;
      }
    } else if (content?.Thinking) {
      const lastMsg = session.messages[session.messages.length - 1];
      if (lastMsg && lastMsg.role === "assistant") {
        if (!lastMsg.thinking) {
          lastMsg.thinking = { content: "", elapsedMs: 0 };
        }
        lastMsg.thinking.content += content.Thinking.thinking ?? "";
        return true;
      }
    }
  } else if (event.ToolCallDelta) {
    const delta = event.ToolCallDelta;
    const lastMsg = session.messages[session.messages.length - 1];
    if (lastMsg && lastMsg.role === "assistant") {
      if (!lastMsg.tools) lastMsg.tools = [];
      let tool = lastMsg.tools.find((t) => t.id === delta.tool_id);
      if (!tool) {
        tool = {
          id: delta.tool_id,
          toolName: delta.tool_name,
          status: "running",
          arguments: "",
          folded: true,
        };
        lastMsg.tools.push(tool);
      }
      if (delta.arguments_delta) {
        tool.arguments = (tool.arguments ?? "") + delta.arguments_delta;
      }
      return true;
    }
  } else if (event.Completed || event.Error) {
    if (session.streaming) {
      session.streaming = false;
      return true;
    }
  }
  return false;
}

function handleToolEvent(session: SessionState, event: any): boolean {
  if (event.Start) {
    const start = event.Start;
    const lastMsg = session.messages[session.messages.length - 1];
    if (lastMsg && lastMsg.role === "assistant") {
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
      } else {
        tool.status = "running";
        if (start.arguments) tool.arguments = start.arguments;
      }
      return true;
    }
  } else if (event.End) {
    const end = event.End;
    const lastMsg = session.messages[session.messages.length - 1];
    if (lastMsg && lastMsg.tools) {
      const tool = lastMsg.tools.find((t) => t.id === end.tool_id);
      if (tool) {
        tool.status = end.is_error ? "failed" : "completed";
        tool.elapsedMs = end.elapsed_ms;
        tool.output = end.content_blocks
          ?.map((b: any) => {
            if (typeof b === "string") return b;
            if (b.Text) return b.Text;
            if (b.text) return b.text;
            return "";
          })
          .join("");
        return true;
      }
    }
  } else if (event.Progress) {
    const progress = event.Progress;
    const lastMsg = session.messages[session.messages.length - 1];
    if (lastMsg && lastMsg.tools) {
      const tool = lastMsg.tools.find((t) => t.id === progress.tool_id);
      if (tool) {
        tool.progress = progress.message;
        tool.tokens = progress.tokens;
        return true;
      }
    }
  }
  return false;
}

function handleAgentEvent(session: SessionState, event: any): boolean {
  if (event.Lifecycle) {
    const state = event.Lifecycle.state;
    if (state === "Running" && !session.streaming) {
      session.streaming = true;
      return true;
    } else if (typeof state === "object") {
      if ((state.TurnCompleted || state.Stopped) && session.streaming) {
        session.streaming = false;
        return true;
      }
    }
  } else if (event.Error && session.streaming) {
    session.streaming = false;
    return true;
  }
  return false;
}

function handleSystemEvent(session: SessionState, event: any): boolean {
  if (event.Shutdown && session.streaming) {
    session.streaming = false;
    return true;
  }
  return false;
}
