import { describe, expect, test } from "vitest";
import {
  estimateJsonTokens,
  estimateTextTokens,
  extractPartialTarget,
  formatRunElapsed,
  formatStreamTokens,
  toolVerb,
} from "./stream-status";

describe("estimateTextTokens", () => {
  test("estimates ASCII text at 4 bytes per token, rounding up", () => {
    expect(estimateTextTokens("hello world")).toBe(3); // ceil(11 / 4)
    expect(estimateTextTokens("abcd")).toBe(1);
  });

  test("counts CJK text by UTF-8 bytes like the kernel", () => {
    expect(estimateTextTokens("你好世界")).toBe(3); // ceil(12 / 4)
  });

  test("empty text is zero", () => {
    expect(estimateTextTokens("")).toBe(0);
  });
});

describe("estimateJsonTokens", () => {
  test("estimates JSON at 2 bytes per token", () => {
    expect(estimateJsonTokens('{"a":1}')).toBe(4); // ceil(7 / 2)
  });
});

describe("formatStreamTokens", () => {
  test("prefixes small counts with ~ and pluralizes", () => {
    expect(formatStreamTokens(1)).toBe("~1 token");
    expect(formatStreamTokens(42)).toBe("~42 tokens");
  });

  test("collapses thousands to one decimal", () => {
    expect(formatStreamTokens(1200)).toBe("~1.2k tokens");
    expect(formatStreamTokens(10500)).toBe("~10.5k tokens");
  });
});

describe("toolVerb", () => {
  test("maps known tools to present-tense verbs", () => {
    expect(toolVerb("edit")).toBe("Editing");
    expect(toolVerb("read")).toBe("Reading");
    expect(toolVerb("write")).toBe("Writing");
    expect(toolVerb("shell")).toBe("Running");
    expect(toolVerb("grep")).toBe("Searching");
    expect(toolVerb("web_fetch")).toBe("Fetching");
    expect(toolVerb("agent")).toBe("Delegating");
    expect(toolVerb("sleep")).toBe("Sleeping");
  });

  test("falls back to Calling for unknown tools", () => {
    expect(toolVerb("mcp__something")).toBe("Calling");
  });
});

describe("extractPartialTarget", () => {
  test("extracts from complete JSON via the strict parser", () => {
    expect(
      extractPartialTarget(
        "edit",
        '{"path": "a.rs", "old_str": "x", "new_str": "y"}',
      ),
    ).toBe("a.rs");
    expect(extractPartialTarget("shell", '{"command": "cargo build"}')).toBe(
      "cargo build",
    );
  });

  test("extracts from truncated JSON while later args still stream", () => {
    expect(
      extractPartialTarget("edit", '{"path": "crates/a.rs", "old_str": "fn ma'),
    ).toBe("crates/a.rs");
  });

  test("picks up a target value whose own string is unterminated", () => {
    expect(extractPartialTarget("read", '{"path": "crates/gu')).toBe(
      "crates/gu",
    );
  });

  test("uses the first target key in argument order", () => {
    expect(
      extractPartialTarget(
        "write",
        '{"file_path": "out.md", "content": "# hi"}',
      ),
    ).toBe("out.md");
  });

  test("returns empty for unknown tools or missing keys", () => {
    expect(extractPartialTarget("mcp__x", '{"path": "a"}')).toBe("");
    expect(extractPartialTarget("edit", "")).toBe("");
    expect(extractPartialTarget("edit", "{}")).toBe("");
  });
});

describe("formatRunElapsed", () => {
  test("bare seconds under a minute", () => {
    expect(formatRunElapsed(0)).toBe("0s");
    expect(formatRunElapsed(8)).toBe("8s");
    expect(formatRunElapsed(59.9)).toBe("59s");
  });

  test("NmNs under an hour", () => {
    expect(formatRunElapsed(60)).toBe("1m0s");
    expect(formatRunElapsed(84)).toBe("1m24s");
  });

  test("NhNmNs beyond an hour", () => {
    expect(formatRunElapsed(3600)).toBe("1h0m0s");
    expect(formatRunElapsed(3753)).toBe("1h2m33s");
  });
});
