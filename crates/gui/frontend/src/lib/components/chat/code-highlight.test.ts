import { describe, expect, test } from "vitest";
import { normalizeCodeLanguage, shouldHighlightCode } from "./code-highlight";

describe("normalizeCodeLanguage", () => {
  test("normalizes common aliases", () => {
    expect(normalizeCodeLanguage("TS")).toBe("typescript");
    expect(normalizeCodeLanguage("py")).toBe("python");
    expect(normalizeCodeLanguage("yml")).toBe("yaml");
  });

  test("falls back to text for an empty language", () => {
    expect(normalizeCodeLanguage(undefined)).toBe("text");
    expect(normalizeCodeLanguage("  ")).toBe("text");
  });
});

describe("shouldHighlightCode", () => {
  test("skips plain text, Mermaid, and oversized sources", () => {
    expect(shouldHighlightCode("value", "text")).toBe(false);
    expect(shouldHighlightCode("graph TD", "mermaid")).toBe(false);
    expect(shouldHighlightCode("x".repeat(100_001), "typescript")).toBe(false);
    expect(
      shouldHighlightCode(
        Array.from({ length: 3_001 }, () => "x").join("\n"),
        "rust",
      ),
    ).toBe(false);
  });

  test("accepts a regular source file", () => {
    expect(shouldHighlightCode("const value = 1;", "typescript")).toBe(true);
  });
});
