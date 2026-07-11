import { expect, test } from "@playwright/test";

test.describe("activity group state", () => {
  test("groups activity only when message ids match", async ({ page }) => {
    await page.goto("/");

    const result = await page.evaluate(async () => {
      const { isSameActivityMessage } =
        await import("/src/lib/components/chat/activity-group.ts");
      const first = {
        id: "message-one",
        type: "tool" as const,
        tool_call_id: "tool-one",
        tool_name: "agent",
        status: "running" as const,
        arguments: "{}",
        result: [],
        created_at: new Date().toISOString(),
      };
      return {
        same: isSameActivityMessage([first], { ...first }),
        different: isSameActivityMessage([first], {
          ...first,
          id: "message-two",
          tool_call_id: "tool-two",
        }),
      };
    });

    expect(result).toEqual({ same: true, different: false });
  });

  test("keeps a tool-calling assistant active after text arrives", async ({
    page,
  }) => {
    await page.goto("/");

    const active = await page.evaluate(async () => {
      const { isActivityTail } =
        await import("/src/lib/components/chat/activity-group.ts");
      return isActivityTail({
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
    });

    expect(active).toBe(true);
  });

  test("does not treat final text without tool calls as active", async ({
    page,
  }) => {
    await page.goto("/");

    const active = await page.evaluate(async () => {
      const { isActivityTail } =
        await import("/src/lib/components/chat/activity-group.ts");
      return isActivityTail({
        id: "assistant-final-text",
        type: "assistant",
        content: [
          { type: "thinking", thinking: "considering" },
          { type: "text", text: "Final answer" },
        ],
        created_at: new Date().toISOString(),
      });
    });

    expect(active).toBe(false);
  });
});
