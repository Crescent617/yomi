import { describe, expect, test } from "vitest";
import { sessionState, streamingMessages } from "./state.svelte";
import { createSessionState } from "./session";
import { isActiveSessionPhase } from "./session-phase";
import {
  buildDisplayItems,
  liveActivityIndex,
} from "./components/chat/display-items";
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
    const groups = buildDisplayItems(messages, true);

    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({
      type: "action_group",
    });
    expect(liveActivityIndex(groups)).toBe(0);
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
    );
    expect(followUpGroups).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: "message",
          isStreaming: true,
        }),
      ]),
    );
    // The follow-up text takes over the tail, so no live activity group.
    expect(liveActivityIndex(followUpGroups)).toBe(-1);
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
  test("deduplicates permission requests and removes all copies on ack", () => {
    const sessionId = "permission-ack-regression";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);

    const request = {
      agent: {
        permission_request: {
          req_id: "permission-1",
          session_id: sessionId,
          tool_name: "shell",
          tool_args: "echo hi",
          tool_level: "caution",
          reason: "test",
        },
      },
    };
    handleEvent(sessionId, "permission-1", request);
    handleEvent(sessionId, "permission-1-replay", request);
    expect(session.pending_permissions).toHaveLength(1);

    handleEvent(sessionId, "permission-ack", {
      agent: {
        permission_ack: { req_id: "permission-1" },
      },
    });
    expect(session.pending_permissions).toHaveLength(0);
  });

  test("deduplicates ask user requests and removes all copies on ack", () => {
    const sessionId = "ask-user-ack-regression";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);

    const request = {
      agent: {
        ask_user_question: {
          req_id: "ask-1",
          session_id: sessionId,
          questions: [],
        },
      },
    };
    handleEvent(sessionId, "ask-1", request);
    handleEvent(sessionId, "ask-1-replay", request);
    expect(session.pending_ask_users).toHaveLength(1);

    handleEvent(sessionId, "ask-ack", {
      agent: {
        ask_user_ack: { req_id: "ask-1" },
      },
    });
    expect(session.pending_ask_users).toHaveLength(0);
  });
});

describe("run output accumulation", () => {
  const textChunk = (messageId: string, bytes: number) => ({
    model: {
      chunk: {
        message_id: messageId,
        content: { text: "y".repeat(bytes) },
      },
    },
  });
  const tokenUsage = (messageId: string, completionTokens: number) => ({
    model: {
      token_usage: {
        message_id: messageId,
        prompt_tokens: 10_000,
        completion_tokens: completionTokens,
        total_tokens: 10_000 + completionTokens,
        context_window: 200_000,
      },
    },
  });

  const modelEnd = (messageId: string) => ({
    model: { end: { message_id: messageId } },
  });

  test("accumulates bytes, folds last usage report at end, resets at Stopped", () => {
    const sessionId = "run-output-accumulation";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    // Run starts; streamed bytes count toward the in-flight estimate.
    handleEvent(sessionId, "running-1", {
      agent: { lifecycle: { state: "running" } },
    });
    handleEvent(sessionId, "request-1", {
      model: { request: { message_id: "m1", message_count: 1 } },
    });
    handleEvent(sessionId, "chunk-1", textChunk("m1", 40));
    handleEvent(sessionId, "delta-1", {
      model: {
        tool_call_delta: {
          message_id: "m1",
          tool_id: "t1",
          tool_name: "bash",
          arguments_delta: "a".repeat(200),
        },
      },
    });
    expect(session.out_stream).toMatchObject({ text: 40, json: 200, run: 0 });

    // Usage is held as pending (last report wins); the end-fold moves it
    // into the run total exactly once.
    handleEvent(sessionId, "usage-1", tokenUsage("m1", 2_000));
    handleEvent(sessionId, "usage-1-final", tokenUsage("m1", 2_345));
    expect(session.out_stream).toMatchObject({
      text: 0,
      json: 0,
      run: 0,
      pending: 2_345,
    });
    handleEvent(sessionId, "end-1", modelEnd("m1"));
    expect(session.out_stream).toMatchObject({ run: 2_345 });
    expect(session.out_stream?.pending).toBeUndefined();

    // Next request: in-flight counters reset, run total carries over.
    handleEvent(sessionId, "request-2", {
      model: { request: { message_id: "m2", message_count: 3 } },
    });
    handleEvent(sessionId, "chunk-2", textChunk("m2", 40));
    expect(session.out_stream).toMatchObject({ run: 2_345, text: 40 });

    // Per-turn Running does not reset mid-run; Stopped does.
    handleEvent(sessionId, "running-2", {
      agent: { lifecycle: { state: "running" } },
    });
    expect(session.out_stream?.run).toBe(2_345);
    handleEvent(sessionId, "stopped", {
      agent: {
        lifecycle: {
          state: { stopped: { reason: { completed: {} } } },
        },
      },
    });
    expect(session.out_stream).toBeUndefined();
  });

  test("a response without a usage report folds its estimate at end", () => {
    const sessionId = "run-output-usageless";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "running", {
      agent: { lifecycle: { state: "running" } },
    });
    handleEvent(sessionId, "chunk-1", textChunk("m1", 400));
    handleEvent(sessionId, "end-1", modelEnd("m1"));
    expect(session.out_stream).toMatchObject({ run: 100, text: 0, json: 0 });
  });

  test("mid-run compaction (message_replaced, no Stopped) keeps the count", () => {
    const sessionId = "run-output-compaction";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "running", {
      agent: { lifecycle: { state: "running" } },
    });
    handleEvent(sessionId, "chunk-1", textChunk("m1", 40));
    handleEvent(sessionId, "usage-1", tokenUsage("m1", 2_345));
    handleEvent(sessionId, "end-1", modelEnd("m1"));

    // Auto-compaction replaces history mid-run: the run never stopped, so
    // the accumulation survives; the next turn keeps adding on top.
    handleEvent(sessionId, "replaced", {
      agent: { message_replaced: null },
    });
    expect(session.out_stream?.run).toBe(2_345);
    handleEvent(sessionId, "chunk-2", textChunk("m2", 40));
    expect(session.out_stream?.text).toBe(40);
  });

  test("a retried request discards the failed attempt's bytes", () => {
    const sessionId = "run-output-retry";
    const session = createSessionState({ id: sessionId });
    sessionState.sessions.push(session);
    streamingMessages[sessionId] = [];

    handleEvent(sessionId, "running", {
      agent: { lifecycle: { state: "running" } },
    });
    handleEvent(sessionId, "request-1", {
      model: { request: { message_id: "m1", message_count: 1 } },
    });
    handleEvent(sessionId, "chunk-1", textChunk("m1", 1_000));
    handleEvent(sessionId, "usage-1", tokenUsage("m1", 500));
    // Stream fails; the retry re-fires Request before re-streaming — the
    // failed attempt's bytes AND pending usage are discarded.
    handleEvent(sessionId, "request-1-retry", {
      model: { request: { message_id: "m1b", message_count: 1 } },
    });
    expect(session.out_stream?.text).toBe(0);
    expect(session.out_stream?.pending).toBeUndefined();
    handleEvent(sessionId, "chunk-1-retry", textChunk("m1b", 40));
    expect(session.out_stream?.text).toBe(40);
  });
});
