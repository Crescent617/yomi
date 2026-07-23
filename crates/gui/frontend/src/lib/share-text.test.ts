import { describe, it, expect } from "vitest";
import { markdownToPlainText, wrapText } from "./share-text";

describe("markdownToPlainText", () => {
  it("strips heading markers", () => {
    expect(markdownToPlainText("# Title\n## Sub")).toBe("Title\nSub");
  });

  it("strips bold, italic and inline code", () => {
    expect(markdownToPlainText("**bold** *em* `code` _u_ ~~del~~")).toBe(
      "bold em code u del",
    );
  });

  it("keeps link text and image alt", () => {
    expect(markdownToPlainText("[docs](https://x.com) ![logo](/img.png)")).toBe(
      "docs logo",
    );
  });

  it("keeps fenced code content but drops fences", () => {
    const md = "before\n```rust\nfn main() {}\n```\nafter";
    expect(markdownToPlainText(md)).toBe("before\nfn main() {}\nafter");
  });

  it("does not treat fence content as markdown", () => {
    const md = "```\n# not a heading\n**not bold**\n```";
    expect(markdownToPlainText(md)).toBe("# not a heading\n**not bold**");
  });

  it("strips blockquotes and horizontal rules", () => {
    expect(markdownToPlainText("> quote\n---\nplain")).toBe("quote\nplain");
  });

  it("collapses excessive blank lines", () => {
    expect(markdownToPlainText("a\n\n\n\n\nb")).toBe("a\n\nb");
  });

  it("strips html tags", () => {
    expect(markdownToPlainText("a<br/>b <b>c</b>")).toBe("ab c");
  });

  it("handles empty input", () => {
    expect(markdownToPlainText("")).toBe("");
  });
});

describe("wrapText", () => {
  // Fixed-width measurer: every char is 1 unit wide.
  const measure = (s: string) => s.length;

  it("returns short text as a single line", () => {
    expect(wrapText("hello", 10, measure)).toEqual(["hello"]);
  });

  it("wraps at word boundaries", () => {
    expect(wrapText("hello world foo", 11, measure)).toEqual([
      "hello",
      "world foo",
    ]);
  });

  it("breaks long tokens anywhere", () => {
    expect(wrapText("abcdefghij", 4, measure)).toEqual(["abcd", "efgh", "ij"]);
  });

  it("preserves hard newlines", () => {
    expect(wrapText("ab\n\ncd", 10, measure)).toEqual(["ab", "", "cd"]);
  });

  it("handles CJK text without spaces", () => {
    expect(wrapText("你好世界你好", 3, measure)).toEqual(["你好世", "界你好"]);
  });

  it("always terminates on tiny widths", () => {
    expect(wrapText("hello", 1, measure)).toEqual(["h", "e", "l", "l", "o"]);
  });
});
