import { describe, expect, it } from "vitest";
import {
  MAX_QUOTE_CHARS,
  composeOutgoingText,
  formatQuoteText,
  quotePrefix,
  quoteSourceMessageId,
} from "./quote";

describe("formatQuoteText", () => {
  it("trims and normalizes line endings", () => {
    expect(formatQuoteText("  hello\r\nworld  ")).toBe("hello\nworld");
  });

  it("collapses blank-line runs", () => {
    expect(formatQuoteText("a\n\n\n\nb")).toBe("a\n\nb");
  });

  it("strips control and PUA characters", () => {
    // F700 = macOS PUA; 0007 = C0
    expect(formatQuoteText("ab\uF700c")).toBe("abc");
    expect(formatQuoteText("x\u0007y")).toBe("xy");
  });

  it("returns empty string for whitespace-only input", () => {
    expect(formatQuoteText("   \n  ")).toBe("");
  });

  it("truncates by code point without splitting surrogate pairs", () => {
    const emoji = "😀"; // 2 UTF-16 code units
    const text = "a".repeat(MAX_QUOTE_CHARS - 1) + emoji + "tail";
    const out = formatQuoteText(text);
    expect(out.endsWith("…")).toBe(true);
    expect(out).not.toContain("�");
    expect([...out].length).toBeLessThanOrEqual(MAX_QUOTE_CHARS + 1);
  });

  it("respects a custom cap", () => {
    expect(formatQuoteText("abcdefgh", 3)).toBe("abc…");
  });
});

describe("quotePrefix", () => {
  it("is empty when there are no quotes", () => {
    expect(quotePrefix([])).toBe("");
    expect(quotePrefix(["", "   "])).toBe("");
  });

  it("prefixes every line of a multi-line quote", () => {
    expect(quotePrefix(["line1\nline2"])).toBe("> line1\n> line2");
  });

  it("keeps blank lines inside the blockquote", () => {
    expect(quotePrefix(["a\n\nb"])).toBe("> a\n> \n> b");
  });

  it("separates multiple quotes with a blank line", () => {
    expect(quotePrefix(["first", "second"])).toBe("> first\n\n> second");
  });
});

describe("composeOutgoingText", () => {
  it("returns base text unchanged without quotes", () => {
    expect(composeOutgoingText([], "fix this")).toBe("fix this");
  });

  it("puts the quote block above the user text", () => {
    expect(composeOutgoingText(["quoted"], "fix this")).toBe(
      "> quoted\n\nfix this",
    );
  });

  it("returns the bare prefix when base text is empty", () => {
    expect(composeOutgoingText(["quoted"], "")).toBe("> quoted");
  });
});

// Minimal structural stand-ins for DOM nodes (vitest runs in node env).
function fakeElement(
  messageId: string | null,
  parent: unknown = null,
): unknown {
  return {
    nodeType: 1,
    parentElement: parent,
    getAttribute: (name: string) =>
      name === "data-message-id" ? messageId : null,
  };
}

function fakeText(parent: unknown): unknown {
  return { nodeType: 3, parentElement: parent };
}

describe("quoteSourceMessageId", () => {
  function selection(anchorNode: unknown, focusNode: unknown): Selection {
    return { anchorNode, focusNode } as Selection;
  }

  it("resolves the containing message for text nodes", () => {
    const msg = fakeElement("m1");
    const node = fakeText(msg);
    expect(quoteSourceMessageId(selection(node, node))).toBe("m1");
  });

  it("walks up nested elements", () => {
    const msg = fakeElement("m2");
    const inner = fakeElement(null, fakeElement(null, msg));
    expect(quoteSourceMessageId(selection(inner, inner))).toBe("m2");
  });

  it("returns null outside message containers", () => {
    const outside = fakeElement(null);
    expect(quoteSourceMessageId(selection(outside, outside))).toBeNull();
  });

  it("returns null for selections spanning two messages", () => {
    const a = fakeText(fakeElement("m1"));
    const b = fakeText(fakeElement("m2"));
    expect(quoteSourceMessageId(selection(a, b))).toBeNull();
  });

  it("returns null when anchor is missing", () => {
    expect(quoteSourceMessageId(selection(null, null))).toBeNull();
  });
});
