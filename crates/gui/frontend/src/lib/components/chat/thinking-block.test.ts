import { describe, expect, test } from "vitest";
import { thinkingPreview } from "./thinking-block";

describe("thinkingPreview", () => {
  test("collapses multiline thinking into a single header preview", () => {
    expect(thinkingPreview("  Compare the options\n\nthen pick one.  ")).toBe(
      "Compare the options then pick one.",
    );
  });

  test("keeps an empty thought empty", () => {
    expect(thinkingPreview(" \n\t ")).toBe("");
  });
});
