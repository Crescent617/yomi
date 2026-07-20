import { describe, expect, test } from "vitest";
import {
  estimateJsonTokens,
  estimateTextTokens,
  formatStreamTokens,
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
