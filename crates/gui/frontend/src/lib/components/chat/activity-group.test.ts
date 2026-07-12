import { describe, expect, test } from "vitest";
import { isActivityTail } from "./activity-group";

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
