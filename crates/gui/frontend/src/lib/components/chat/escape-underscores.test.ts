import { describe, expect, it } from "vitest";
import { escapeIntrawordUnderscores } from "./escape-underscores";

describe("escapeIntrawordUnderscores", () => {
  it("escapes snake_case identifiers", () => {
    expect(escapeIntrawordUnderscores("unknown finish_reason repeat")).toBe(
      "unknown finish\\_reason repeat",
    );
  });

  it("escapes every run inside a word", () => {
    expect(escapeIntrawordUnderscores("x_y_z")).toBe("x\\_y\\_z");
    expect(escapeIntrawordUnderscores("a__b")).toBe("a\\_\\_b");
  });

  it("keeps real emphasis delimiters", () => {
    expect(escapeIntrawordUnderscores("_foo_ bar")).toBe("_foo_ bar");
    expect(escapeIntrawordUnderscores("__bold__ text")).toBe("__bold__ text");
  });

  it("keeps emphasis containing snake_case, matching CommonMark", () => {
    // The inner underscore is intra-word, so it must not terminate the span.
    expect(escapeIntrawordUnderscores("_foo_bar_")).toBe("_foo\\_bar_");
  });

  it("leaves word-boundary underscores alone", () => {
    expect(escapeIntrawordUnderscores("_reason")).toBe("_reason");
    expect(escapeIntrawordUnderscores("reason_")).toBe("reason_");
  });

  it("does not escape inside fenced code blocks", () => {
    const md = "before\n```\nlet a_b = 1;\n```\nafter_c";
    expect(escapeIntrawordUnderscores(md)).toBe(
      "before\n```\nlet a_b = 1;\n```\nafter\\_c",
    );
    expect(escapeIntrawordUnderscores("~~~\nx_y\n~~~")).toBe("~~~\nx_y\n~~~");
  });

  it("does not escape inside inline code spans", () => {
    expect(escapeIntrawordUnderscores("`a_b` and c_d")).toBe("`a_b` and c\\_d");
    expect(escapeIntrawordUnderscores("``a_b ` c``")).toBe("``a_b ` c``");
  });

  it("does not escape inside link destinations", () => {
    expect(escapeIntrawordUnderscores("[t](https://a.com/b_c) d_e")).toBe(
      "[t](https://a.com/b_c) d\\_e",
    );
  });

  it("does not escape inside bare URLs", () => {
    // An escaped underscore would truncate the renderer's raw-URL token.
    expect(
      escapeIntrawordUnderscores(
        "见 https://dev.example.com/delivery/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix-metrics 备用",
      ),
    ).toBe(
      "见 https://dev.example.com/delivery/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix-metrics 备用",
    );
    // Escaping resumes right after the URL.
    expect(escapeIntrawordUnderscores("https://a.com/b_c then d_e")).toBe(
      "https://a.com/b_c then d\\_e",
    );
  });

  it("stops the bare URL at whitespace, backslash, quotes and angle brackets", () => {
    expect(escapeIntrawordUnderscores("https://a.com/b_c\nnext_line")).toBe(
      "https://a.com/b_c\nnext\\_line",
    );
    expect(escapeIntrawordUnderscores('"https://a.com/b_c" d_e')).toBe(
      '"https://a.com/b_c" d\\_e',
    );
    expect(escapeIntrawordUnderscores("<https://a.com/b_c> d_e")).toBe(
      "<https://a.com/b_c> d\\_e",
    );
    // `]`, `)` and trailing punctuation stay inside by design: the
    // renderer's raw-URL token includes them too, so protection mirrors
    // what will actually be linkified.
    expect(escapeIntrawordUnderscores("(https://a.com/b_c) d_e")).toBe(
      "(https://a.com/b_c) d\\_e",
    );
  });

  it("escapes underscores inside CJK words too", () => {
    expect(escapeIntrawordUnderscores("变量_名 测试")).toBe("变量\\_名 测试");
    expect(escapeIntrawordUnderscores("hello_世界")).toBe("hello\\_世界");
  });
});
