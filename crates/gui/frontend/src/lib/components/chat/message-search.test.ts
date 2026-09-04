import { describe, expect, it } from "vitest";
import {
  clampActiveIndex,
  countMatches,
  findMatches,
  stepActiveIndex,
} from "./message-search";
import type { Message } from "../../state.svelte";

function msg(id: string, text: string): Message {
  return {
    id,
    type: "assistant",
    content: [{ type: "text", text }],
    timestamp: 0,
  } as unknown as Message;
}

describe("countMatches", () => {
  it("empty query matches nothing", () => {
    expect(countMatches("hello hello", "")).toBe(0);
  });

  it("is case-insensitive and counts every occurrence", () => {
    expect(countMatches("Foo foo FOO", "foo")).toBe(3);
    expect(countMatches("nothing here", "foo")).toBe(0);
  });

  it("does not count overlapping occurrences", () => {
    expect(countMatches("aaaa", "aa")).toBe(2);
    expect(countMatches("aaa", "aa")).toBe(1);
  });
});

describe("findMatches", () => {
  it("flattens matches in message order with per-message ordinals", () => {
    const matches = findMatches(
      [msg("a", "x and x"), msg("b", "none"), msg("c", "x")],
      "x",
    );
    expect(matches).toEqual([
      { message_id: "a", occurrence: 0 },
      { message_id: "a", occurrence: 1 },
      { message_id: "c", occurrence: 0 },
    ]);
  });

  it("blank query matches nothing", () => {
    expect(findMatches([msg("a", "x")], "   ")).toEqual([]);
  });

  it("matches across text blocks within one message", () => {
    const message = {
      id: "m",
      type: "assistant",
      content: [
        { type: "text", text: "foo " },
        { type: "image_url", image_url: { url: "data:..." } },
        { type: "text", text: "bar foo" },
      ],
      timestamp: 0,
    } as unknown as Message;
    expect(findMatches([message], "foo")).toEqual([
      { message_id: "m", occurrence: 0 },
      { message_id: "m", occurrence: 1 },
    ]);
  });
});

describe("index math", () => {
  it("clamps stale indexes into range", () => {
    expect(clampActiveIndex(5, 3)).toBe(2);
    expect(clampActiveIndex(-1, 3)).toBe(0);
    expect(clampActiveIndex(0, 0)).toBe(0);
  });

  it("wraps around on both ends", () => {
    expect(stepActiveIndex(2, 3, 1)).toBe(0);
    expect(stepActiveIndex(0, 3, -1)).toBe(2);
    expect(stepActiveIndex(1, 3, 1)).toBe(2);
    expect(stepActiveIndex(0, 0, 1)).toBe(0);
  });
});
