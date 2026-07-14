import { describe, expect, test } from "vitest";
import { formatBytes, prependEarlierContent } from "./debug-viewer";

describe("debug viewer", () => {
  test("prepends raw content", () => {
    expect(prependEarlierContent("first\n", "last\n")).toBe("first\nlast\n");
  });

  test("formats file sizes", () => {
    expect(formatBytes(12)).toBe("12 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(2 * 1024 * 1024)).toBe("2.0 MB");
  });
});
