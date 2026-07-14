import { describe, expect, test } from "vitest";
import { elapsedLabel, shellActivitySummary } from "./status-activity";

describe("status activity", () => {
  test("summarizes running shells", () => {
    expect(shellActivitySummary(1)).toBe("Shells 1");
    expect(shellActivitySummary(3)).toBe("Shells 3");
  });

  test("formats elapsed runtime", () => {
    const now = Date.parse("2026-07-14T12:02:05Z");
    expect(elapsedLabel("2026-07-14T12:02:00Z", now)).toBe("5s");
    expect(elapsedLabel("2026-07-14T12:00:00Z", now)).toBe("2m");
    expect(elapsedLabel("2026-07-14T10:00:00Z", now)).toBe("2h 2m");
  });
});
