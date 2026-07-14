import { describe, expect, test } from "vitest";
import {
  diffSources,
  highlightDiffHunks,
  resolveDiffLanguagePath,
  tokenStyle,
} from "./diff-highlight";

describe("diffSources", () => {
  test("rebuilds old and new sources from a hunk", () => {
    const lines = [
      { type: "context" as const, text: "const value = 1;" },
      { type: "del" as const, text: "console.log(value);" },
      { type: "add" as const, text: "return value;" },
    ];

    expect(diffSources(lines)).toEqual({
      oldSource: "const value = 1;\nconsole.log(value);",
      newSource: "const value = 1;\nreturn value;",
    });
  });
  test("highlights independently selected files with different languages", async () => {
    const first = {
      lines: [{ type: "add" as const, text: "const value: number = 1;" }],
    };
    const second = {
      lines: [{ type: "add" as const, text: "fn value() -> i32 { 1 }" }],
    };

    await highlightDiffHunks([first], "src/first.ts");
    await highlightDiffHunks([second], "src/second.rs");

    expect(first.lines[0].newTokens?.length).toBeGreaterThan(0);
    expect(second.lines[0].newTokens?.length).toBeGreaterThan(0);
  });
});

describe("resolveDiffLanguagePath", () => {
  test("uses the old path for deleted files", () => {
    expect(
      resolveDiffLanguagePath("src/removed.ts", "/dev/null", "removed.ts"),
    ).toBe("src/removed.ts");
  });

  test("uses each selected file fallback when diff paths are unavailable", () => {
    expect(resolveDiffLanguagePath(undefined, undefined, "src/next.rs")).toBe(
      "src/next.rs",
    );
  });
});

describe("tokenStyle", () => {
  test("preserves light and dark Shiki token colors", () => {
    expect(
      tokenStyle({
        content: "const",
        offset: 0,
        htmlStyle: { color: "#000", "--shiki-dark": "#fff" },
      }),
    ).toBe("color:#000;--shiki-dark:#fff");
  });
});
