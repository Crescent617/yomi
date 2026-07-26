import { describe, expect, test } from "vitest";
import { aggregateMood, elapsedLabel, moodTextClass } from "./status-activity";

describe("status activity", () => {
  test("formats elapsed runtime", () => {
    const now = Date.parse("2026-07-14T12:02:05Z");
    expect(elapsedLabel("2026-07-14T12:02:00Z", now)).toBe("5s");
    expect(elapsedLabel("2026-07-14T12:00:00Z", now)).toBe("2m");
    expect(elapsedLabel("2026-07-14T10:00:00Z", now)).toBe("2h 2m");
  });
});

describe("aggregateMood", () => {
  test("permission requests outrank everything", () => {
    expect(
      aggregateMood({
        pendingPermission: true,
        pendingAsk: true,
        runningCount: 2,
      }),
    ).toBe("alert");
  });

  test("ask-user questions outrank running work", () => {
    expect(
      aggregateMood({
        pendingPermission: false,
        pendingAsk: true,
        runningCount: 1,
      }),
    ).toBe("curious");
  });

  test("running sessions mean working", () => {
    expect(
      aggregateMood({
        pendingPermission: false,
        pendingAsk: false,
        runningCount: 3,
      }),
    ).toBe("working");
  });

  test("nothing going on is idle", () => {
    expect(
      aggregateMood({
        pendingPermission: false,
        pendingAsk: false,
        runningCount: 0,
      }),
    ).toBe("idle");
  });
});

describe("moodTextClass", () => {
  test("maps every mood to a semantic text class", () => {
    expect(moodTextClass("alert")).toBe("text-error");
    expect(moodTextClass("curious")).toBe("text-info");
    expect(moodTextClass("working")).toBe("text-primary");
    expect(moodTextClass("idle")).toBe("text-muted-foreground");
  });
});
