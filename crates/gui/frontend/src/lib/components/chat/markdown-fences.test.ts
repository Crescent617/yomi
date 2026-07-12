import { describe, expect, test } from "vitest";
import { countClosedMermaidFences } from "./markdown-fences";

describe("countClosedMermaidFences", () => {
  test("counts only closed Mermaid fences", () => {
    expect(countClosedMermaidFences("```mermaid\ngraph TD; A-->B")).toBe(0);
    expect(countClosedMermaidFences("```mermaid\ngraph TD; A-->B\n```")).toBe(
      1,
    );
  });

  test("handles multiple fence styles and ignores other languages", () => {
    const markdown = [
      "```ts",
      "const value = 1;",
      "```",
      "~~~ Mermaid extra",
      "graph TD; A-->B",
      "~~~~",
      "```mermaid",
      "graph LR; C-->D",
      "```",
    ].join("\n");

    expect(countClosedMermaidFences(markdown)).toBe(2);
  });

  test("does not close a fence with a shorter or mismatched marker", () => {
    const markdown = [
      "````mermaid",
      "graph TD; A-->B",
      "```",
      "~~~",
      "````",
    ].join("\n");

    expect(countClosedMermaidFences(markdown)).toBe(1);
  });
});
