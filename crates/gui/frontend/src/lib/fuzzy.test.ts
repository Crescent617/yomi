import { describe, expect, it } from "vitest";
import { fuzzyFilter, fuzzyScore } from "./fuzzy";

describe("fuzzyScore", () => {
  it("empty query matches everything with score 0", () => {
    expect(fuzzyScore("", "anything")).toBe(0);
    expect(fuzzyScore("   ", "anything")).toBe(0);
  });

  it("returns null when the query is not a subsequence", () => {
    expect(fuzzyScore("xyz", "fork session")).toBeNull();
    expect(fuzzyScore("abc", "acb")).toBeNull();
  });

  it("is case-insensitive", () => {
    expect(fuzzyScore("FORK", "fork session")).not.toBeNull();
    expect(fuzzyScore("fork", "FORK SESSION")).not.toBeNull();
  });

  it("prefers consecutive runs over scattered hits", () => {
    const run = fuzzyScore("fork", "fork session");
    const scattered = fuzzyScore("fork", "f o r k");
    expect(run).not.toBeNull();
    expect(scattered).not.toBeNull();
    expect(run!).toBeGreaterThan(scattered!);
  });

  it("prefers word-start hits", () => {
    const wordStart = fuzzyScore("sess", "my session");
    const midWord = fuzzyScore("sess", "assessment");
    expect(wordStart).not.toBeNull();
    expect(midWord).not.toBeNull();
    expect(wordStart!).toBeGreaterThan(midWord!);
  });

  it("prefers shorter candidates for the same query", () => {
    const short = fuzzyScore("del", "delete");
    const long = fuzzyScore("del", "delete everything in the whole world");
    expect(short!).toBeGreaterThan(long!);
  });

  it("matches CJK text", () => {
    expect(fuzzyScore("会话", "新建会话")).not.toBeNull();
    expect(fuzzyScore("新会", "新建会话")).not.toBeNull();
  });
});

describe("fuzzyFilter", () => {
  const items = ["fork session", "delete session", "restart kernel"];

  it("empty query passes through in input order", () => {
    expect(fuzzyFilter("", items, (s) => s)).toEqual(items);
  });

  it("ranks by score and drops non-matches", () => {
    const out = fuzzyFilter("sess", items, (s) => s);
    expect(out).toHaveLength(2);
    expect(out).toContain("fork session");
    expect(out).toContain("delete session");
  });

  it("returns empty when nothing matches", () => {
    expect(fuzzyFilter("zzz", items, (s) => s)).toEqual([]);
  });
});
