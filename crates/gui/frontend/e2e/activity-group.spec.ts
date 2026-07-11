import { expect, test } from "@playwright/test";

test.describe("activity group state", () => {
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
