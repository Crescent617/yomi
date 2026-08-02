import * as api from "./api";
import {
  sessionState,
  getSession,
  showNotification,
  streamingMessages,
  type SessionState,
  type Message,
  type BotMessage,
  type ToolMessage,
  type ModelChunk,
  type ToolEvent,
  type AgentEvent,
  type UserEvent,
  type KernelEvent,
} from "./state.svelte";
import { ensureSessionPhase, setSessionPhase } from "./session-phase";
import { estimateStreamTokens, utf8Length } from "./tokens";
import {
  sendDesktopNotification,
  refreshCheckpoints,
  appendSessionMessages,
  loadSessionMessages,
} from "./session";

// ── Event helpers ────────────────────────────────────────────────────────

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

function findToolMessage(
  session: SessionState,
  message_id: string,
): ToolMessage | undefined {
  const message = findMessageById(session, message_id);
  return message?.type === "tool" ? message : undefined;
}

function warnToolIdentityMismatch(
  session: SessionState,
  message_id: string,
  tool_id: string,
) {
  const allMessages = [
    ...session.messages,
    ...(streamingMessages[session.id] ?? []),
  ];
  const existing = allMessages.find(
    (message) => message.type === "tool" && message.tool_call_id === tool_id,
  );
  if (existing && existing.id !== message_id) {
    console.warn("Tool events reused a tool_id with a different message_id", {
      session_id: session.id,
      tool_id,
      existing_message_id: existing.id,
      event_message_id: message_id,
    });
  }
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
        current.git_refresh_revision = (current.git_refresh_revision ?? 0) + 1;
      }
    })
    .catch(() => {
      const current = getSession(id);
      if (current && current.id === sessionState.activeSessionId) {
        current.git_info = null;
        current.git_refresh_revision = (current.git_refresh_revision ?? 0) + 1;
      }
    });
}

function maybeRefreshTodos(session: SessionState, toolName: string) {
  if (toolName === "todo") {
    api
      .getTodos(session.id)
      .then((r) => {
        session.todos = r.todos;
      })
      .catch(() => {});
  }
}

// ── Main event dispatcher ──────────────────────────────────────────────

export function handleEvent(
  session_id: string,
  event_id: string | undefined,
  rawEvent: unknown,
) {
  const session = getSession(session_id);
  if (!session) return;

  const ev = rawEvent as KernelEvent;
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

// ── Model events ───────────────────────────────────────────────────────

/** The session's run-output counters, created on first streamed byte. */
function outStream(session: SessionState) {
  return (session.out_stream ??= { text: 0, json: 0, run: 0 });
}

function handleModelEvent(session: SessionState, event: ModelChunk): boolean {
  if (event.chunk || event.tool_call_delta) {
    ensureSessionPhase(session, "streaming");
  }

  if (event.token_usage) {
    const u = event.token_usage;
    session.token_usage = {
      prompt_tokens: u.prompt_tokens,
      completion_tokens: u.completion_tokens,
      total_tokens: u.total_tokens,
    };
    // Hold the real output for the end-fold; providers may report usage
    // multiple times per response — the last report wins (as the kernel).
    const s = outStream(session);
    s.pending = u.completion_tokens;
    s.text = 0;
    s.json = 0;
    return true;
  }

  if (event.chunk) {
    const chunk = event.chunk;
    const content = chunk.content;

    if (content?.text) {
      const text = content.text;
      outStream(session).text += utf8Length(text);
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
      outStream(session).text += utf8Length(thinkingText);
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
      return true;
    }
    return true;
  } else if (event.tool_call_delta) {
    const delta = event.tool_call_delta;
    outStream(session).json += utf8Length(delta.arguments_delta ?? "");
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
    showNotification(`Model error: ${err.error}`, "error");
    return false;
  } else if (event.request) {
    // New model call: discard the in-flight counters — a retried attempt
    // restarts here, so its partial output is never double-counted.
    const s = outStream(session);
    s.text = 0;
    s.json = 0;
    s.pending = undefined;
    return true;
  } else if (event.end) {
    // Response finished: fold the real completion when reported, the
    // estimate otherwise (usage-less providers). Success-only — a failed
    // attempt's counters were already discarded at the retry's request.
    const s = outStream(session);
    s.run += s.pending ?? estimateStreamTokens(s.text, s.json);
    s.pending = undefined;
    s.text = 0;
    s.json = 0;
    return true;
  } else if (event.fallback) {
    const fb = event.fallback;
    showNotification(`Fallback from ${fb.from} to ${fb.to}`, "info");
    return true;
  }
  return false;
}

// ── Tool events ────────────────────────────────────────────────────────

function handleToolEvent(session: SessionState, event: ToolEvent): boolean {
  if (event.start) {
    const start = event.start;
    const msg = findToolMessage(session, start.message_id);
    if (msg) {
      msg.status = "running";
      if (start.arguments) msg.arguments = start.arguments;
      return true;
    }
    warnToolIdentityMismatch(session, start.message_id, start.tool_id);
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
    const msg = findToolMessage(session, md.message_id);
    if (msg) {
      const sid = md.metadata["subagent_session_id"];
      if (sid) {
        msg.subagent_session_id = sid;
      }
      return true;
    }
    warnToolIdentityMismatch(session, md.message_id, md.tool_id);
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
    const msg = findToolMessage(session, end.message_id);
    if (msg) {
      msg.status = end.is_error ? "failed" : "completed";
      msg.elapsed_ms = end.elapsed_ms;
      msg.result = end.content_blocks ?? [];
      maybeRefreshTodos(session, end.tool_name);
      maybeRefreshGitInfo(session);
      return true;
    }
    warnToolIdentityMismatch(session, end.message_id, end.tool_id);
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

// ── Agent events ───────────────────────────────────────────────────────

function handleAgentEvent(session: SessionState, event: AgentEvent): boolean {
  if (event.state_changed) {
    setSessionPhase(session, event.state_changed.state);
    return true;
  }

  if (event.lifecycle) {
    const state = event.lifecycle.state;
    if (state === "running") {
      setSessionPhase(session, "streaming");
      return true;
    } else if (typeof state === "object" && state.stopped) {
      setSessionPhase(session, "idle");
      // The run is over — reset the output accumulation. Resetting here
      // (not at the next Running) keeps the count across mid-run
      // compaction, which fires MessageReplaced but never Stopped.
      session.out_stream = undefined;
      const buf = streamingMessages[session.id] ?? [];
      if (buf.length > 0) {
        const seen = new Set(session.messages.map((m) => m.id));
        const deduped = buf.filter((m) => !seen.has(m.id));
        if (deduped.length > 0) {
          appendSessionMessages(session, deduped);
        }
        streamingMessages[session.id] = [];
      }
      refreshCheckpoints(session.id);
      const stopReason = state.stopped.reason;
      if ("cancelled" in stopReason) {
        const op = stopReason.cancelled.operation;
        const msg = op ? `Cancelled: ${op}` : "Cancelled";
        appendSessionMessages(session, [
          {
            id: crypto.randomUUID(),
            type: "error",
            content: msg,
            created_at: new Date().toISOString(),
          },
        ]);
        showNotification(msg, "warning");
        sendDesktopNotification("Yomi", msg, session.id);
        return true;
      } else if ("failed" in stopReason) {
        const errorMsg =
          "Task failed: " + (stopReason.failed.error ?? "Unknown");
        appendSessionMessages(session, [
          {
            id: crypto.randomUUID(),
            type: "error",
            content: errorMsg,
            created_at: new Date().toISOString(),
          },
        ]);
        showNotification(errorMsg, "warning");
        sendDesktopNotification("Yomi", errorMsg, session.id);
        return true;
      } else if ("max_iterations" in stopReason) {
        const msg = `Max iterations reached (${stopReason.max_iterations.reached})`;
        appendSessionMessages(session, [
          {
            id: crypto.randomUUID(),
            type: "error",
            content: msg,
            created_at: new Date().toISOString(),
          },
        ]);
        showNotification(msg, "warning");
        sendDesktopNotification("Yomi", msg, session.id);
        return true;
      }
      sendDesktopNotification("Yomi", "Task completed", session.id);
      return true;
    }
  } else if (event.error) {
    const buf = streamingMessages[session.id] ?? [];
    if (buf.length > 0) {
      const seen = new Set(session.messages.map((m) => m.id));
      const deduped = buf.filter((m) => !seen.has(m.id));
      if (deduped.length > 0) {
        appendSessionMessages(session, deduped);
      }
      streamingMessages[session.id] = [];
    }
    const errorStr = event.error.error ?? "Unknown";
    const errorMsg = "Agent error: " + errorStr;
    appendSessionMessages(session, [
      {
        id: crypto.randomUUID(),
        type: "error",
        content: errorMsg,
        created_at: new Date().toISOString(),
      },
    ]);
    // Non-recoverable errors are NOT always followed by a Stopped::Failed
    // lifecycle event (the kernel may recover to Idle), so surface both.
    const level = event.error.is_recoverable ? "warning" : "error";
    showNotification(errorMsg, level);
    if (!event.error.is_recoverable) {
      sendDesktopNotification("Yomi", errorMsg, session.id);
    }
    return true;
  } else if (event.retrying) {
    const retry = event.retrying;
    const msg = `Agent retrying (${retry.attempt}/${retry.max_attempts})`;
    appendSessionMessages(session, [
      {
        id: crypto.randomUUID(),
        type: "error",
        content: msg,
        created_at: new Date().toISOString(),
      },
    ]);
    showNotification(msg, "warning");
    return true;
  } else if (event.permission_request) {
    const req = event.permission_request;
    session.pending_permissions = [
      ...session.pending_permissions.filter(
        (item) => item.req_id !== req.req_id,
      ),
      {
        req_id: req.req_id,
        session_id: req.session_id,
        tool_name: req.tool_name,
        tool_args: req.tool_args ?? "",
        tool_level: req.tool_level ?? "safe",
        reason: req.reason ?? "",
      },
    ];
    showNotification(`${req.tool_name} needs approval`, "warning");
    return true;
  } else if (event.ask_user_question) {
    const req = event.ask_user_question;
    session.pending_ask_users = [
      ...session.pending_ask_users.filter((item) => item.req_id !== req.req_id),
      {
        req_id: req.req_id,
        session_id: req.session_id,
        questions: req.questions,
      },
    ];
    showNotification("Agent has a question for you", "info");
    sendDesktopNotification("Yomi", "Agent has a question for you", session.id);
    return true;
  } else if (event.permission_ack) {
    const req_id = event.permission_ack.req_id;
    const idx = session.pending_permissions.findIndex(
      (p) => p.req_id === req_id,
    );
    if (idx >= 0) {
      session.pending_permissions = session.pending_permissions.filter(
        (item) => item.req_id !== req_id,
      );
    }
    return true;
  } else if (event.ask_user_ack) {
    const req_id = event.ask_user_ack.req_id;
    const idx = session.pending_ask_users.findIndex((a) => a.req_id === req_id);
    if (idx >= 0) {
      session.pending_ask_users = session.pending_ask_users.filter(
        (item) => item.req_id !== req_id,
      );
    }
    return true;
  } else if (event.message_replaced !== undefined) {
    streamingMessages[session.id] = [];
    setSessionPhase(session, "idle");
    api
      .getMessages(session.id)
      .then((msgs) => loadSessionMessages(session.id, msgs))
      .catch((e: Error) =>
        console.error("Failed to reload messages after MessageReplaced:", e),
      );
    refreshCheckpoints(session.id);
    showNotification("Session message replaced", "info");
    return true;
  } else if (event.goal_updated) {
    session.goal = {
      description: event.goal_updated.description,
      status: event.goal_updated.status,
    };
    return true;
  } else if (event.goal_stopped !== undefined) {
    session.goal = null;
    return true;
  }
  return false;
}

// ── User events ────────────────────────────────────────────────────────

function handleUserEvent(session: SessionState, event: UserEvent): boolean {
  if (event.message) {
    const msg = event.message;
    appendSessionMessages(session, [
      {
        id: msg.message_id,
        type: "user",
        content: msg.content ?? [],
        created_at: new Date().toISOString(),
      },
    ]);
    session.updated_at = new Date().toISOString();
    return true;
  }
  if (event.steer) {
    const msg = event.steer;
    const buf = streamingMessages[session.id] ?? [];
    buf.push({
      id: msg.message_id,
      type: "steer",
      content: msg.content ?? [],
      created_at: new Date().toISOString(),
    });
    streamingMessages[session.id] = buf;
    session.updated_at = new Date().toISOString();
    return true;
  }
  return false;
}
