import { describe, expect, it } from "vitest";
import {
  escapeIntrawordUnderscores,
  IncrementalUnderscoreEscape,
} from "./escape-underscores";

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
        "见 https://example.com/delivery/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix-metrics 备用",
      ),
    ).toBe(
      "见 https://example.com/delivery/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix-metrics 备用",
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

describe("IncrementalUnderscoreEscape", () => {
  /** Feed `md` to the escaper in `step`-sized appends, collecting outputs. */
  function streamOutputs(md: string, step: number): string[] {
    const escaper = new IncrementalUnderscoreEscape();
    const outputs: string[] = [];
    for (let end = step; end < md.length; end += step) {
      outputs.push(escaper.update(md.slice(0, end)));
    }
    outputs.push(escaper.update(md));
    return outputs;
  }

  const samples = [
    "plain text without markers",
    "unknown finish_reason repeat",
    "snake_case and `code_span_x` and _real_emphasis_",
    "```py\ndef foo_bar():\n    return baz_qux\n```\nafter_fence_x",
    "~~~\nx_y\n~~~",
    "partial fence\n `` \n ```js\ncode_x\n ``` \nout_x",
    "[t](https://a.com/b_c) d_e and https://a.com/f_g then h_i",
    "多行\n变量_名 测试\nhello_世界\n最后一行 partial_x",
    "trailing newline kept\nnext_line_下划线",
  ];

  it("matches the one-shot result at every append step", () => {
    for (const md of samples) {
      for (const step of [1, 3, 7]) {
        const outputs = streamOutputs(md, step);
        for (let i = 0; i < outputs.length; i++) {
          const prefix =
            i === outputs.length - 1
              ? md
              : md.slice(
                  0,
                  (i + 1) * step < md.length ? (i + 1) * step : md.length,
                );
          expect(outputs[i]).toBe(escapeIntrawordUnderscores(prefix));
        }
      }
    }
  });

  it("handles a fence marker arriving character by character", () => {
    const escaper = new IncrementalUnderscoreEscape();
    const md = "before_x\n```\nin_fence_y\n```\nafter_z";
    let last = "";
    for (let end = 1; end <= md.length; end++) {
      last = escaper.update(md.slice(0, end));
      expect(last).toBe(escapeIntrawordUnderscores(md.slice(0, end)));
    }
    expect(last).toBe("before\\_x\n```\nin_fence_y\n```\nafter\\_z");
  });

  it("falls back to a full pass when the input is not an append", () => {
    const escaper = new IncrementalUnderscoreEscape();
    escaper.update("aaa_bbb\nccc_ddd\neee_");
    expect(escaper.update("%%MARKER%%\nfff_ggg")).toBe(
      escapeIntrawordUnderscores("%%MARKER%%\nfff_ggg"),
    );
    expect(escaper.update("a_b")).toBe("a\\_b");
  });

  it("returns identical output for repeated identical input", () => {
    const escaper = new IncrementalUnderscoreEscape();
    const first = escaper.update("some_text\nmore_text");
    expect(escaper.update("some_text\nmore_text")).toBe(first);
  });
});
