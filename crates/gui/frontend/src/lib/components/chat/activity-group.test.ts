import { describe, expect, test } from "vitest";
import { isActivityTail, isAgentActivity } from "./activity-group";

describe("isAgentActivity", () => {
  test("recognizes an agent tool before subagent metadata arrives", () => {
    expect(
      isAgentActivity({
        id: "tool-agent",
        type: "tool",
        tool_call_id: "call-agent",
        tool_name: "agent",
        status: "executing",
        arguments: "{}",
        result: [],
      }),
    ).toBe(true);
  });

  test("recognizes legacy agent activity by subagent session metadata", () => {
    expect(
      isAgentActivity({
        id: "tool-legacy",
        type: "tool",
        tool_call_id: "call-legacy",
        tool_name: "sub_agent",
        status: "completed",
        arguments: "{}",
        result: [],
        subagent_session_id: "sess_child",
      }),
    ).toBe(true);
  });

  test("does not classify regular tools as agents", () => {
    expect(
      isAgentActivity({
        id: "tool-shell",
        type: "tool",
        tool_call_id: "call-shell",
        tool_name: "shell",
        status: "completed",
        arguments: "{}",
        result: [],
      }),
    ).toBe(false);
  });
});

describe("activity group state", () => {
  test("keeps a tool-calling assistant active after text arrives", () => {
    const active = isActivityTail({
      id: "assistant-with-tool",
      type: "assistant",
      content: [
        { type: "thinking", thinking: "considering" },
        { type: "text", text: "Calling a tool" },
      ],
      tool_calls: [
        { id: "tool-call", name: "read", arguments: '{"path":"a"}' },
      ],
      created_at: new Date().toISOString(),
    });

    expect(active).toBe(true);
  });

  test("does not treat final text without tool calls as active", () => {
    const active = isActivityTail({
      id: "assistant-final-text",
      type: "assistant",
      content: [
        { type: "thinking", thinking: "considering" },
        { type: "text", text: "Final answer" },
      ],
      created_at: new Date().toISOString(),
    });

    expect(active).toBe(false);
  });
});
