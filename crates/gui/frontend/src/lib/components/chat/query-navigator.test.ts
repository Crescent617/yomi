import type { Message, UserMessage } from "../../state.svelte";
import { describe, expect, test } from "vitest";
import { summarizeUserQuery, userQueryMarkers } from "./query-navigator";

const created_at = "2026-01-01T00:00:00.000Z";

function user(id: string, text: string): UserMessage {
  return {
    id,
    type: "user",
    content: [{ type: "text", text }],
    created_at,
  };
}

describe("query minimap", () => {
  test("includes only user queries", () => {
    const messages: Message[] = [
      user("u1", "first query"),
      {
        id: "s1",
        type: "steer",
        content: [{ type: "text", text: "change direction" }],
        created_at,
      },
      {
        id: "a1",
        type: "assistant",
        content: [{ type: "text", text: "answer" }],
        created_at,
      },
      user("u2", "second query"),
    ];

    expect(userQueryMarkers(messages)).toEqual([
      { id: "u1", label: "first query" },
      { id: "u2", label: "second query" },
    ]);
  });

  test("removes reminders, whitespace, and truncates long labels", () => {
    const summary = summarizeUserQuery(
      `  explain   this\n<system_reminder>internal context</system_reminder> ${"x".repeat(90)}`,
    );

    expect(summary).not.toContain("internal context");
    expect(summary).not.toContain("  ");
    expect(Array.from(summary)).toHaveLength(72);
    expect(summary.endsWith("…")).toBe(true);
  });

  test("labels image-only queries", () => {
    const messages: Message[] = [
      {
        id: "u1",
        type: "user",
        content: [{ type: "image_url", image_url: { url: "asset://image" } }],
        created_at,
      },
    ];

    expect(userQueryMarkers(messages)).toEqual([
      { id: "u1", label: "Image attachment" },
    ]);
  });
});
