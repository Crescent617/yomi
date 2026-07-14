import { describe, expect, test } from "vitest";
import {
  appendNewContent,
  formatBytes,
  prependEarlierContent,
} from "./debug-viewer";

describe("debug viewer", () => {
  test("appends only bytes after the previous offset", () => {
    expect(
      appendNewContent("a你", 4, {
        content: "你b",
        start_offset: 1,
        end_offset: 5,
      }),
    ).toBe("a你b");
  });

  test("replaces content when chunks do not overlap", () => {
    expect(
      appendNewContent("old", 3, {
        content: "new",
        start_offset: 10,
        end_offset: 13,
      }),
    ).toBe("new");
  });

  test("prepends raw content", () => {
    expect(prependEarlierContent("first\n", "last\n")).toBe("first\nlast\n");
  });

  test("formats file sizes", () => {
    expect(formatBytes(12)).toBe("12 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(2 * 1024 * 1024)).toBe("2.0 MB");
  });
});
