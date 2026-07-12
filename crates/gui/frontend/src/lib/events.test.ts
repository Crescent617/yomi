import { describe, expect, test } from "vitest";
import { sessionState, streamingMessages } from "./state.svelte";
import { createSessionState } from "./session";
import { handleEvent } from "./events";

describe("tool event streaming", () => {
  test("merges agent events by message id", () => {
    const sessionId = "agent-stream-regression";
    const messageId = "agent-tool-message";
    const session = createSessionState({ id: sessionId });

    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "event-start", {
      tool: {
        start: {
          message_id: messageId,
          tool_id: "agent-call",
          tool_name: "agent",
          arguments: '{"description":"review"}',
        },
      },
    });
    handleEvent(sessionId, "event-meta", {
      tool: {
        metadata: {
          message_id: messageId,
          tool_id: "agent-call",
          metadata: { subagent_session_id: "subagent-session" },
        },
      },
    });
    handleEvent(sessionId, "event-end", {
      tool: {
        end: {
          message_id: messageId,
          tool_id: "agent-call",
          tool_name: "agent",
          is_error: false,
          elapsed_ms: 42,
          content_blocks: [{ type: "text", text: "done" }],
        },
      },
    });

    const tools = (streamingMessages[sessionId] ?? []).filter(
      (message) => message.type === "tool",
    );
    const tool = tools[0];

    expect({
      count: tools.length,
      id: tool?.id,
      status: tool?.type === "tool" ? tool.status : undefined,
      subagentSessionId:
        tool?.type === "tool" ? tool.subagent_session_id : undefined,
    }).toEqual({
      count: 1,
      id: "agent-tool-message",
      status: "completed",
      subagentSessionId: "subagent-session",
    });
  });

  test("does not merge different message ids that reuse a tool id", () => {
    const sessionId = "tool-id-reuse-regression";
    const session = createSessionState({ id: sessionId });

    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "event-meta", {
      tool: {
        metadata: {
          message_id: "metadata-message",
          tool_id: "reused-tool-call",
          metadata: {},
        },
      },
    });
    handleEvent(sessionId, "event-end", {
      tool: {
        end: {
          message_id: "result-message",
          tool_id: "reused-tool-call",
          tool_name: "agent",
          is_error: false,
          elapsed_ms: 42,
          content_blocks: [],
        },
      },
    });

    const count = (streamingMessages[sessionId] ?? []).filter(
      (message) => message.type === "tool",
    ).length;

    expect(count).toBe(2);
  });
});
