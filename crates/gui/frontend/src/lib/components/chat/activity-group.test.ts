import { describe, expect, test } from "vitest";
import {
  buildActivityTrail,
  categorizeToolName,
  computeActivityStats,
  isAgentActivity,
} from "./activity-group";

describe("isAgentActivity", () => {
  test("recognizes an agent tool before subagent metadata arrives", () => {
    expect(
      isAgentActivity({
        id: "tool-agent",
        type: "tool",
        tool_call_id: "call-agent",
        tool_name: "agent",
        status: "running",
        arguments: "{}",
        result: [],
        created_at: new Date().toISOString(),
      }),
    ).toBe(true);
  });

  test("does not use subagent metadata to classify another tool", () => {
    expect(
      isAgentActivity({
        id: "tool-with-metadata",
        type: "tool",
        tool_call_id: "call-with-metadata",
        tool_name: "shell",
        status: "completed",
        arguments: "{}",
        result: [],
        subagent_session_id: "sess_child",
        created_at: new Date().toISOString(),
      }),
    ).toBe(false);
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
        created_at: new Date().toISOString(),
      }),
    ).toBe(false);
  });
});

describe("categorizeToolName", () => {
  test.each([
    ["read", "searchRead"],
    ["web_search", "searchRead"],
    ["webSearch", "searchRead"],
    ["edit", "editWrite"],
    ["shell", "shell"],
    ["agent", "agent"],
    ["post_message", "other"],
    ["my_custom_tool", "other"],
  ])("categorizes %s as %s", (name, category) => {
    expect(categorizeToolName(name)).toBe(category);
  });
});

describe("unmaterialized tool calls", () => {
  const assistantWithPendingCalls = {
    id: "assistant-pending",
    type: "assistant" as const,
    content: [{ type: "thinking", thinking: "planning" }],
    tool_calls: [
      { id: "call-read", name: "read", arguments: '{"path":"a"}' },
      { id: "call-agent", name: "agent", arguments: "{}" },
    ],
    created_at: new Date().toISOString(),
  };

  test("are neither counted nor rendered before tool messages arrive", () => {
    const stats = computeActivityStats([assistantWithPendingCalls]);
    expect(stats.thinkingCount).toBe(1);
    expect(stats.searchReadCount).toBe(0);
    expect(stats.subagentCount).toBe(0);
    expect(stats.otherToolCount).toBe(0);
    expect(stats.actionCount).toBe(1);

    const trail = buildActivityTrail([assistantWithPendingCalls]);
    expect(trail.map((item) => item.type)).toEqual(["thought"]);
  });

  test("tool messages count under their own categories once materialized", () => {
    const readTool = {
      id: "tool-read",
      type: "tool" as const,
      tool_call_id: "call-read",
      tool_name: "read",
      status: "completed" as const,
      arguments: '{"path":"a"}',
      result: [],
      created_at: new Date().toISOString(),
    };
    const agentTool = {
      id: "tool-agent",
      type: "tool" as const,
      tool_call_id: "call-agent",
      tool_name: "agent",
      status: "running" as const,
      arguments: "{}",
      result: [],
      created_at: new Date().toISOString(),
    };

    const stats = computeActivityStats([
      assistantWithPendingCalls,
      readTool,
      agentTool,
    ]);
    expect(stats.searchReadCount).toBe(1);
    expect(stats.subagentCount).toBe(1);
    expect(stats.otherToolCount).toBe(0);
    expect(stats.actionCount).toBe(3);

    const trail = buildActivityTrail([assistantWithPendingCalls, readTool]);
    expect(trail.map((item) => item.type)).toEqual(["thought", "tool"]);
  });
});
