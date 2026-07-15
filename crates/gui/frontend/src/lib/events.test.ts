import { describe, expect, test } from "vitest";
import { sessionState, streamingMessages } from "./state.svelte";
import { createSessionState } from "./session";
import { isActiveSessionPhase } from "./session-phase";
import { buildDisplayItems } from "./components/chat/display-items";
import { handleEvent } from "./events";

describe("tool event streaming", () => {
  test("restores streaming phase when model output follows a stale idle state", () => {
    const sessionId = "model-after-tool-regression";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "model-output", {
      model: {
        chunk: {
          message_id: "assistant-message",
          content: { text: "still writing" },
        },
      },
    });

    handleEvent(sessionId, "more-model-output", {
      model: {
        chunk: {
          message_id: "assistant-message",
          content: { text: " more" },
        },
      },
    });

    expect(session.phase).toBe("streaming");
    expect(session.phase_revision).toBe(1);
    expect(streamingMessages[sessionId]).toHaveLength(1);
  });

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

  test("keeps streaming visible after an activity group completes", () => {
    const sessionId = "activity-follow-up-regression";
    const session = createSessionState({ id: sessionId });

    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "phase-tool", {
      agent: { state_changed: { state: "executing_tool" } },
    });
    handleEvent(sessionId, "tool-start", {
      tool: {
        start: {
          message_id: "tool-message",
          tool_id: "tool-call",
          tool_name: "read",
          arguments: '{"path":"README.md"}',
        },
      },
    });
    handleEvent(sessionId, "tool-end", {
      tool: {
        end: {
          message_id: "tool-message",
          tool_id: "tool-call",
          tool_name: "read",
          is_error: false,
          elapsed_ms: 42,
          content_blocks: [{ type: "text", text: "done" }],
        },
      },
    });
    handleEvent(sessionId, "phase-streaming", {
      agent: { state_changed: { state: "streaming" } },
    });

    const messages = streamingMessages[sessionId] ?? [];
    const groups = buildDisplayItems(messages, true, true);

    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({
      type: "action_group",
      isActiveActivity: true,
    });
    expect(isActiveSessionPhase(session.phase)).toBe(true);

    handleEvent(sessionId, "follow-up-chunk", {
      model: {
        chunk: {
          message_id: "assistant-follow-up",
          content: { text: "Continuing after the tool" },
        },
      },
    });

    expect(isActiveSessionPhase(session.phase)).toBe(true);
    expect(streamingMessages[sessionId]?.at(-1)).toMatchObject({
      id: "assistant-follow-up",
      type: "assistant",
    });

    const followUpGroups = buildDisplayItems(
      streamingMessages[sessionId] ?? [],
      true,
      true,
    );
    expect(followUpGroups).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: "message",
          isStreaming: true,
        }),
      ]),
    );
    expect(
      followUpGroups.some(
        (item) =>
          item.type === "action_group" && item.isActiveActivity === true,
      ),
    ).toBe(false);
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
