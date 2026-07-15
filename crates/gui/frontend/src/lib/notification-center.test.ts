import { describe, expect, test } from "vitest";
import {
  MAX_SESSION_NOTIFICATIONS,
  addSessionCompletion,
  didSessionComplete,
  relativeNotificationTime,
  seedRunningSessionStatuses,
  type SessionCompletionNotification,
} from "./notification-center";

function notification(index: number): SessionCompletionNotification {
  return {
    id: `notification-${index}`,
    sessionId: `session-${index}`,
    title: `Session ${index}`,
    projectId: null,
    completedAt: new Date(index).toISOString(),
    read: false,
  };
}

describe("session completion notifications", () => {
  test("only records a transition into idle", () => {
    expect(didSessionComplete(undefined, "idle")).toBe(false);
    expect(didSessionComplete("idle", "idle")).toBe(false);
    expect(didSessionComplete("streaming", "executing_tool")).toBe(false);
    expect(didSessionComplete("streaming", "idle")).toBe(true);
    expect(didSessionComplete("executing_tool", "idle")).toBe(true);
  });

  test("running session snapshots seed completion baselines", () => {
    const statuses = new Map<string, string>([["known", "executing_tool"]]);
    seedRunningSessionStatuses(statuses, ["known", "reconnected"]);

    expect(statuses.get("known")).toBe("executing_tool");
    expect(statuses.get("reconnected")).toBe("streaming");
    expect(didSessionComplete(statuses.get("reconnected"), "idle")).toBe(true);
  });

  test("keeps only the latest notification for each session", () => {
    const older = notification(1);
    const other = notification(2);
    const latest = {
      ...notification(3),
      sessionId: older.sessionId,
      title: "Latest completion",
      read: true,
    };

    const items = addSessionCompletion([other, older], latest);

    expect(items).toEqual([latest, other]);
  });

  test("keeps newest notifications first and caps history", () => {
    let items: SessionCompletionNotification[] = [];
    for (let index = 0; index < MAX_SESSION_NOTIFICATIONS + 5; index += 1) {
      items = addSessionCompletion(items, notification(index));
    }
    expect(items).toHaveLength(MAX_SESSION_NOTIFICATIONS);
    expect(items[0].id).toBe(`notification-${MAX_SESSION_NOTIFICATIONS + 4}`);
    expect(items.at(-1)?.id).toBe("notification-5");
  });

  test("formats compact relative time", () => {
    const now = Date.parse("2026-07-15T02:00:00Z");
    expect(relativeNotificationTime("2026-07-15T01:59:40Z", now)).toBe("now");
    expect(relativeNotificationTime("2026-07-15T01:45:00Z", now)).toBe("15m");
    expect(relativeNotificationTime("2026-07-14T23:00:00Z", now)).toBe("3h");
    expect(relativeNotificationTime("2026-07-13T02:00:00Z", now)).toBe("2d");
  });
});
