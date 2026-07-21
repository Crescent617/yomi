import { describe, it, expect } from "vitest";
import {
  parseMarkdown,
  parseInline,
  type Block,
  type InlineRun,
} from "./share-markdown";

/** Shorthand: strip styles, keep text. */
function texts(runs: InlineRun[]): string[] {
  return runs.map((r) => r.text);
}

function kinds(blocks: Block[]): string[] {
  return blocks.map((b) => b.kind);
}

describe("parseMarkdown blocks", () => {
  it("parses headings and clamps level to 3", () => {
    const blocks = parseMarkdown("# a\n### c\n##### e");
    expect(kinds(blocks)).toEqual(["heading", "heading", "heading"]);
    expect(blocks.map((b) => (b.kind === "heading" ? b.level : 0))).toEqual([
      1, 3, 3,
    ]);
    expect(texts(blocks[0].kind === "heading" ? blocks[0].runs : [])).toEqual([
      "a",
    ]);
  });

  it("joins soft-wrapped paragraph lines with a space", () => {
    const blocks = parseMarkdown("hello\nworld");
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe("paragraph");
    expect(texts(blocks[0].kind === "paragraph" ? blocks[0].runs : [])).toEqual(
      ["hello world"],
    );
  });

  it("joins CJK lines without a space", () => {
    const blocks = parseMarkdown("你好\n世界");
    const runs = blocks[0].kind === "paragraph" ? blocks[0].runs : [];
    expect(texts(runs)).toEqual(["你好世界"]);
  });

  it("splits paragraphs on blank lines", () => {
    const blocks = parseMarkdown("a\n\n\nb");
    expect(kinds(blocks)).toEqual(["paragraph", "paragraph"]);
  });

  it("parses fenced code and ignores markdown inside", () => {
    const md = "before\n```rust\n# not heading\n**not bold**\n```\nafter";
    const blocks = parseMarkdown(md);
    expect(kinds(blocks)).toEqual(["paragraph", "code", "paragraph"]);
    expect(blocks[1].kind === "code" ? blocks[1].text : "").toBe(
      "# not heading\n**not bold**",
    );
  });

  it("handles unterminated fences", () => {
    const blocks = parseMarkdown("```\ncode");
    expect(kinds(blocks)).toEqual(["code"]);
    expect(blocks[0].kind === "code" ? blocks[0].text : "").toBe("code");
  });

  it("parses unordered lists", () => {
    const blocks = parseMarkdown("- a\n- b\ntail");
    expect(kinds(blocks)).toEqual(["list", "paragraph"]);
    const list = blocks[0];
    if (list.kind !== "list") throw new Error("expected list");
    expect(list.ordered).toBe(false);
    expect(list.items.map((it) => it.label)).toEqual(["•", "•"]);
    expect(list.items.map((it) => it.runs[0]?.text)).toEqual(["a", "b"]);
  });

  it("parses ordered lists keeping source numbers", () => {
    const blocks = parseMarkdown("3. a\n4. b");
    const list = blocks[0];
    if (list.kind !== "list") throw new Error("expected list");
    expect(list.ordered).toBe(true);
    expect(list.items.map((it) => it.label)).toEqual(["3.", "4."]);
  });

  it("merges consecutive quote lines", () => {
    const blocks = parseMarkdown("> a\n> b\ntail");
    expect(kinds(blocks)).toEqual(["quote", "paragraph"]);
    const quote = blocks[0];
    if (quote.kind !== "quote") throw new Error("expected quote");
    expect(texts(quote.runs)).toEqual(["a b"]);
  });

  it("parses horizontal rules", () => {
    expect(kinds(parseMarkdown("a\n\n---\n\nb"))).toEqual([
      "paragraph",
      "hr",
      "paragraph",
    ]);
  });

  it("parses tables with header, alignment and rows", () => {
    const md =
      "| 名称 | 数量 | 备注 |\n| :--- | ---: | :---: |\n| apple | 3 | **fresh** |\n| pear | 10 | - |";
    const blocks = parseMarkdown(md);
    expect(kinds(blocks)).toEqual(["table"]);
    const table = blocks[0];
    if (table.kind !== "table") throw new Error("expected table");
    expect(table.align).toEqual(["left", "right", "center"]);
    expect(table.header.map((c) => c[0]?.text)).toEqual([
      "名称",
      "数量",
      "备注",
    ]);
    expect(table.rows).toHaveLength(2);
    expect(table.rows[0].map((c) => c.map((r) => r.text).join(""))).toEqual([
      "apple",
      "3",
      "fresh",
    ]);
    expect(table.rows[0][2][0]?.style.bold).toBe(true);
  });

  it("pads short table rows to the header width", () => {
    const blocks = parseMarkdown("| a | b |\n| - | - |\n| only |");
    const table = blocks[0];
    if (table.kind !== "table") throw new Error("expected table");
    expect(table.rows[0]).toHaveLength(2);
    expect(table.rows[0][1]).toEqual([]);
  });

  it("does not treat pipe lines without a separator as a table", () => {
    const blocks = parseMarkdown("| a | b |\nnot a table");
    expect(kinds(blocks)).toEqual(["paragraph"]);
  });
});

describe("parseInline", () => {
  it("parses bold", () => {
    expect(parseInline("**bold**")).toEqual([
      { text: "bold", style: { bold: true } },
    ]);
  });

  it("parses italic", () => {
    expect(parseInline("*em*")).toEqual([
      { text: "em", style: { italic: true } },
    ]);
  });

  it("parses bold+italic with triple markers", () => {
    expect(parseInline("***both***")).toEqual([
      { text: "both", style: { bold: true, italic: true } },
    ]);
  });

  it("parses nested italic inside bold", () => {
    expect(parseInline("**a *b* c**")).toEqual([
      { text: "a ", style: { bold: true } },
      { text: "b", style: { bold: true, italic: true } },
      { text: " c", style: { bold: true } },
    ]);
  });

  it("parses inline code without interpreting markers", () => {
    expect(parseInline("`x**y`")).toEqual([
      { text: "x**y", style: { code: true } },
    ]);
  });

  it("parses links and keeps the label", () => {
    expect(parseInline("[docs](https://x.com)")).toEqual([
      { text: "docs", style: { link: true } },
    ]);
  });

  it("keeps image alt text as plain runs", () => {
    expect(parseInline("![logo](/img.png)")).toEqual([
      { text: "logo", style: {} },
    ]);
  });

  it("parses strikethrough", () => {
    expect(parseInline("~~gone~~")).toEqual([
      { text: "gone", style: { strike: true } },
    ]);
  });

  it("leaves snake_case untouched", () => {
    expect(parseInline("foo_bar_baz")).toEqual([
      { text: "foo_bar_baz", style: {} },
    ]);
  });

  it("leaves unmatched markers literal", () => {
    expect(parseInline("*oops")).toEqual([{ text: "*oops", style: {} }]);
  });

  it("handles backslash escapes", () => {
    expect(parseInline("\\*x")).toEqual([{ text: "*x", style: {} }]);
  });

  it("strips html tags", () => {
    expect(parseInline("a<br/>b")).toEqual([{ text: "ab", style: {} }]);
  });

  it("merges adjacent same-style runs", () => {
    expect(parseInline("a<b>b</b>c")).toEqual([{ text: "abc", style: {} }]);
  });

  it("mixes plain and styled text", () => {
    expect(parseInline("use `pnpm dev` now")).toEqual([
      { text: "use ", style: {} },
      { text: "pnpm dev", style: { code: true } },
      { text: " now", style: {} },
    ]);
  });
});
