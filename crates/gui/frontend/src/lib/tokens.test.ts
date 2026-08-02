import { describe, expect, test } from "vitest";
import { estimateStreamTokens, utf8Length } from "./tokens";

describe("utf8Length", () => {
  test("counts UTF-8 bytes, not chars", () => {
    expect(utf8Length("hello world")).toBe(11);
    expect(utf8Length("你好世界")).toBe(12);
    expect(utf8Length("")).toBe(0);
  });
});

describe("estimateStreamTokens", () => {
  test("combines text bytes at 4/token and json bytes at 2/token", () => {
    expect(estimateStreamTokens(400, 200)).toBe(200); // 100 + 100
    expect(estimateStreamTokens(0, 0)).toBe(0);
    expect(estimateStreamTokens(1, 1)).toBe(2); // ceil(1/4) + ceil(1/2)
  });
});
