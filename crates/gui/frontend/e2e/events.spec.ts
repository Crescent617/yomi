import { expect, test } from "@playwright/test";

test.describe("tool event streaming", () => {
  test("merges agent events by message id", async ({ page }) => {
    await page.goto("/");

    const result = await page.evaluate(async () => {
      const state = await import("/src/lib/state.svelte.ts");
      const events = await import("/src/lib/events.ts");
      const sessionId = "agent-stream-regression";
      const messageId = "agent-tool-message";
      const session = state.createSessionState({ id: sessionId });

      state.sessionState.sessions.push(session);
      state.streamingMessages[sessionId] = [];

      events.handleEvent(sessionId, "event-start", {
        tool: {
          start: {
            message_id: messageId,
            tool_id: "agent-call",
            tool_name: "agent",
            arguments: '{"description":"review"}',
          },
        },
      });
      events.handleEvent(sessionId, "event-meta", {
        tool: {
          metadata: {
            message_id: messageId,
            tool_id: "agent-call",
            metadata: { subagent_session_id: "subagent-session" },
          },
        },
      });
      events.handleEvent(sessionId, "event-end", {
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

      const tools = (state.streamingMessages[sessionId] ?? []).filter(
        (message) => message.type === "tool",
      );
      const tool = tools[0];
      return {
        count: tools.length,
        id: tool?.id,
        status: tool?.type === "tool" ? tool.status : undefined,
        subagentSessionId:
          tool?.type === "tool" ? tool.subagent_session_id : undefined,
      };
    });

    expect(result).toEqual({
      count: 1,
      id: "agent-tool-message",
      status: "completed",
      subagentSessionId: "subagent-session",
    });
  });

  test("does not merge different message ids that reuse a tool id", async ({
    page,
  }) => {
    await page.goto("/");

    const count = await page.evaluate(async () => {
      const state = await import("/src/lib/state.svelte.ts");
      const events = await import("/src/lib/events.ts");
      const sessionId = "tool-id-reuse-regression";
      const session = state.createSessionState({ id: sessionId });

      state.sessionState.sessions.push(session);
      state.streamingMessages[sessionId] = [];

      events.handleEvent(sessionId, "event-meta", {
        tool: {
          metadata: {
            message_id: "metadata-message",
            tool_id: "reused-tool-call",
            metadata: {},
          },
        },
      });
      events.handleEvent(sessionId, "event-end", {
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

      return (state.streamingMessages[sessionId] ?? []).filter(
        (message) => message.type === "tool",
      ).length;
    });

    expect(count).toBe(2);
  });
});
