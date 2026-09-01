import { describe, expect, test } from "vitest";
import { parseTokenCount } from "./utils";

describe("parseTokenCount", () => {
  test("plain numbers and unit suffixes", () => {
    expect(parseTokenCount("200000")).toBe(200_000);
    expect(parseTokenCount("512k")).toBe(512_000);
    expect(parseTokenCount("1.5k")).toBe(1500);
    expect(parseTokenCount("1m")).toBe(1_000_000);
    expect(parseTokenCount(" 800K ")).toBe(800_000);
  });

  test("rejects malformed, zero, and negative", () => {
    expect(parseTokenCount("")).toBeNull();
    expect(parseTokenCount("abc")).toBeNull();
    expect(parseTokenCount("0")).toBeNull();
    expect(parseTokenCount("-5k")).toBeNull();
    expect(parseTokenCount("k")).toBeNull();
  });

  test("rejects values above u32::MAX instead of a bare IPC failure", () => {
    expect(parseTokenCount("4294967295")).toBe(0xffffffff);
    expect(parseTokenCount("4294967296")).toBeNull();
    expect(parseTokenCount("9999m")).toBeNull();
  });
});
