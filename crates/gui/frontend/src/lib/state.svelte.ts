import * as api from "./api";

export interface Tab {
  id: string;
  type: "chat" | "preview" | "edit";
  label: string;
  entry?: { name: string; path: string; isDirectory: boolean; isFile: boolean };
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
}

export const appState = $state({
  connectionStatus: "disconnected" as "connected" | "disconnected" | "connecting",
  currentTheme: "system" as "light" | "dark" | "system",
  sidebarCollapsed: false,
  rightPanelCollapsed: true,
});

export const projectState = $state({
  projects: [] as ProjectState[],
  activeProjectId: null as string | null,
});

// Per-project session cursors for pagination
export const sessionCursors = $state(new Map<string, string | null>());

// ── UI notification state (for InfoBar inline notifications) ──
export const uiState = $state<{
  notification: { text: string; level: "info" | "warn" | "error" | "success" } | null;
}>({
  notification: null,
});

let _notificationTimeout: ReturnType<typeof setTimeout> | null = null;

export function showNotification(
  text: string,
  level: "info" | "warn" | "error" | "success" = "info",
  durationMs = 4000
) {
  uiState.notification = { text, level };
  if (_notificationTimeout) clearTimeout(_notificationTimeout);
  _notificationTimeout = setTimeout(() => {
    uiState.notification = null;
  }, durationMs);
}

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

function normalizeRole(role: any): "user" | "tool" | "system" | "assistant" {
  if (role === "User" || role === "user") return "user";
  if (role === "tool" || role === "Tool") return "tool";
  if (role === "system" || role === "System") return "system";
  return "assistant";
}

export function loadSessionMessages(sessionId: string, rawMessages: any[]) {
  const session = getSession(sessionId);
  if (!session) return;

  // First pass: collect all tool outputs from tool result messages
  const toolOutputs: Record<string, string> = {};
  const toolOutputByName: Record<string, string> = {};
  for (const m of rawMessages) {
    const role = normalizeRole(m.role);
    if (role !== "tool") continue;

    let output = "";
    if (Array.isArray(m.content)) {
      output = m.content.map((block: any) => {
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
      const cleanId = m.tool_call_id.replace(/^functions\./, '');
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
  for (const m of rawMessages) {
    const role = normalizeRole(m.role);

    if (role === "tool") {
      continue;
    }

    if (role === "user") {
      parsedMessages.push({
        id: extractId(m.id),
        role: "user",
        content: typeof m.content === "string" ? m.content : Array.isArray(m.content) ? m.content.map((b: any) => b.Text || b.text || "").join("") : "",
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
            thinking = { content: block.thinking || block.Thinking?.thinking || "", elapsedMs: 0 };
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
          const output = toolOutputs[toolId] || toolOutputs[toolId.replace(/^functions\./, '')] || toolOutputByName[toolName.toLowerCase()] || "";
          const hasOutput = output !== "" || toolId in toolOutputs || toolId.replace(/^functions\./, '') in toolOutputs || toolName.toLowerCase() in toolOutputByName;
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

// Handle raw kernel events (serialized Event enum)
export function handleEvent(sessionId: string, rawEvent: any) {
  const session = getSession(sessionId);
  if (!session) return;

  if (rawEvent.Model) {
    handleModelEvent(session, rawEvent.Model);
  } else if (rawEvent.Agent) {
    handleAgentEvent(session, rawEvent.Agent);
  } else if (rawEvent.System) {
    handleSystemEvent(session, rawEvent.System);
  } else if (rawEvent.Tool) {
    handleToolEvent(session, rawEvent.Tool);
  }

  if (sessionState.activeSessionId !== sessionId) {
    session.unread++;
  }
}

/** Search all messages for a tool with the given id. */
function findToolById(session: SessionState, toolId: string): { msg: ChatMessage; tool: ToolCall } | null {
  for (let i = session.messages.length - 1; i >= 0; i--) {
    const msg = session.messages[i];
    if (msg.role === "assistant" && msg.tools) {
      const tool = msg.tools.find((t) => t.id === toolId);
      if (tool) return { msg, tool };
    }
  }
  return null;
}

function handleModelEvent(session: SessionState, event: any): boolean {
  if (event.Chunk) {
    const chunk = event.Chunk;
    const content = chunk.content;
    if (content?.Text) {
      const text = content.Text;
      const lastMsg = session.messages[session.messages.length - 1];
      // Append only if the last assistant message is a pure text block
      // (no thinking, no tools). Otherwise start a new message.
      if (
        lastMsg &&
        lastMsg.role === "assistant" &&
        !lastMsg.thinking &&
        (!lastMsg.tools || lastMsg.tools.length === 0)
      ) {
        lastMsg.content += text;
      } else {
        session.messages = [
          ...session.messages,
          {
            id: crypto.randomUUID(),
            role: "assistant",
            content: text,
            thinking: null,
            tools: [],
          },
        ];
      }
      if (!session.streaming) {
        session.streaming = true;
      }
      return true;
    } else if (content?.Thinking) {
      const lastMsg = session.messages[session.messages.length - 1];
      // Append only if the last assistant message is a pure thinking block
      // (empty content, no tools). Otherwise start a new message.
      if (
        lastMsg &&
        lastMsg.role === "assistant" &&
        !lastMsg.content &&
        (!lastMsg.tools || lastMsg.tools.length === 0)
      ) {
        if (!lastMsg.thinking) {
          lastMsg.thinking = { content: "", elapsedMs: 0 };
        }
        lastMsg.thinking.content += content.Thinking.thinking ?? "";
      } else {
        session.messages = [
          ...session.messages,
          {
            id: crypto.randomUUID(),
            role: "assistant",
            content: "",
            thinking: { content: content.Thinking.thinking ?? "", elapsedMs: 0 },
            tools: [],
          },
        ];
      }
      if (!session.streaming) {
        session.streaming = true;
      }
      return true;
    }
  } else if (event.ToolCallDelta) {
    const delta = event.ToolCallDelta;
    let lastMsg = session.messages[session.messages.length - 1];
    if (!lastMsg || lastMsg.role !== "assistant") {
      // 创建新的 assistant 消息
      session.messages = [
        ...session.messages,
        { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] },
      ];
      lastMsg = session.messages[session.messages.length - 1];
    }
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
    if (!session.streaming) {
      session.streaming = true;
    }
    return true;
  } else if (event.Completed || event.Error) {
    if (session.streaming) {
      session.streaming = false;
      // Auto-send queued message when streaming ends
      if (session.queuedInput) {
        const text = session.queuedInput;
        session.queuedInput = null;
        session.messages = [
          ...session.messages,
          { id: crypto.randomUUID(), role: "user", content: text, thinking: null, tools: [] },
        ];
        api.sendMessage(session.id, text).catch((e) => {
          console.error("Failed to send queued message:", e);
        });
      }
      return true;
    }
  }
  return false;
}

function handleToolEvent(session: SessionState, event: any): boolean {
  if (event.Start) {
    const start = event.Start;
    const found = findToolById(session, start.tool_id);
    if (found) {
      found.tool.status = "running";
      if (start.arguments) found.tool.arguments = start.arguments;
      return true;
    }
    let lastMsg = session.messages[session.messages.length - 1];
    if (!lastMsg || lastMsg.role !== "assistant") {
      // 创建新的 assistant 消息来承载 tool
      session.messages = [
        ...session.messages,
        { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] },
      ];
      lastMsg = session.messages[session.messages.length - 1];
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
    return true;
  } else if (event.End) {
    const end = event.End;
    const found = findToolById(session, end.tool_id);
    if (found) {
      found.tool.status = end.is_error ? "failed" : "completed";
      found.tool.elapsedMs = end.elapsed_ms;
      found.tool.output = end.content_blocks
        ?.map((b: any) => {
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
    let lastMsg = session.messages[session.messages.length - 1];
    if (!lastMsg || lastMsg.role !== "assistant") {
      // 没有 assistant 消息，创建一个空的
      session.messages = [
        ...session.messages,
        { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] },
      ];
      lastMsg = session.messages[session.messages.length - 1];
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
      ?.map((b: any) => {
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
  } else if (event.Progress) {
    const progress = event.Progress;
    const found = findToolById(session, progress.tool_id);
    if (found) {
      found.tool.progress = progress.message;
      found.tool.tokens = progress.tokens;
      return true;
    }
    let lastMsg = session.messages[session.messages.length - 1];
    if (!lastMsg || lastMsg.role !== "assistant") {
      session.messages = [
        ...session.messages,
        { id: crypto.randomUUID(), role: "assistant", content: "", thinking: null, tools: [] },
      ];
      lastMsg = session.messages[session.messages.length - 1];
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
    return true;
  }
  return false;
}

function handleAgentEvent(session: SessionState, event: any): boolean {
  if (event.Lifecycle) {
    const state = event.Lifecycle.state;
    if (state === "Running" && !session.streaming) {
      session.streaming = true;
      showNotification("AI is responding...", "info", 2000);
      return true;
    } else if (typeof state === "object") {
      if ((state.TurnCompleted || state.Stopped) && session.streaming) {
        session.streaming = false;
        return true;
      }
    }
  } else if (event.Error && session.streaming) {
    session.streaming = false;
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
  } else if (event.AskUserQuestion) {
    const q = event.AskUserQuestion;
    session.pendingAskUser = {
      reqId: q.req_id,
      agentId: q.agent_id,
      questions: q.questions ?? [],
    };
    showNotification("AI has a question for you", "info", 5000);
    return true;
  }
  return false;
}

function handleSystemEvent(session: SessionState, event: any): boolean {
  if (event.Shutdown && session.streaming) {
    session.streaming = false;
    showNotification("Session ended", "info", 3000);
    return true;
  }
  return false;
}
