import { describe, expect, test } from "vitest";
import {
  groupSessionsByTime,
  projectSessionsForList,
  topLevelSessionsForList,
} from "./session-time-groups";

const NOW = new Date(2026, 6, 13, 12).getTime();

function session(id: string, updated_at?: string) {
  return { id, updated_at };
}

function minutesAgo(minutes: number): string {
  return new Date(NOW - minutes * 60 * 1000).toISOString();
}

describe("projectSessionsForList", () => {
  test("keeps only top-level sessions assigned to a project", () => {
    const projectSession = { id: "project", project_id: "project_1" };

    expect(
      projectSessionsForList([
        projectSession,
        { id: "default", project_id: null },
        {
          id: "subagent",
          project_id: "project_1",
          parent_session_id: "project",
        },
      ]),
    ).toEqual([projectSession]);
  });
});

describe("topLevelSessionsForList", () => {
  test("keeps sessions without a project, drops subagents", () => {
    const projectSession = { id: "project", project_id: "project_1" };
    const orphanSession = { id: "orphan", project_id: null };

    expect(
      topLevelSessionsForList([
        projectSession,
        orphanSession,
        {
          id: "subagent",
          project_id: "project_1",
          parent_session_id: "project",
        },
      ]),
    ).toEqual([projectSession, orphanSession]);
  });
});

describe("session time groups", () => {
  test("groups recent sessions into fine-grained time windows", () => {
    const groups = groupSessionsByTime(
      [
        session("thirty", minutesAgo(10)),
        session("hour", minutesAgo(45)),
        session("three-hours", minutesAgo(120)),
        session("twelve-hours", minutesAgo(240)),
      ],
      NOW,
    );

    expect(groups.map((group) => group.label)).toEqual([
      null,
      "30 minutes ago",
      "1 hour ago",
      "3 hours ago",
    ]);
    expect(groups.map((group) => group.sessions.map(({ id }) => id))).toEqual([
      ["thirty"],
      ["hour"],
      ["three-hours"],
      ["twelve-hours"],
    ]);
  });

  test("uses local calendar boundaries for older sessions", () => {
    const calendarNow = new Date(2026, 6, 13, 20).getTime();
    const groups = groupSessionsByTime(
      [
        session("today", new Date(2026, 6, 13, 0, 1).toISOString()),
        session("yesterday", new Date(2026, 6, 12, 23, 59).toISOString()),
        session("week", new Date(2026, 6, 7, 12).toISOString()),
        session("month", new Date(2026, 5, 20, 12).toISOString()),
        session("older", new Date(2026, 4, 1, 12).toISOString()),
        session("missing"),
      ],
      calendarNow,
    );

    expect(groups.map((group) => group.label)).toEqual([
      "12 hours ago",
      "Yesterday",
      "A week ago",
      "A month ago",
      "Older",
    ]);
    expect(groups.map((group) => group.sessions.map(({ id }) => id))).toEqual([
      ["today"],
      ["yesterday"],
      ["week"],
      ["month"],
      ["older", "missing"],
    ]);
  });

  test("places a time divider before sessions older than its boundary", () => {
    const groups = groupSessionsByTime(
      [session("newer", minutesAgo(5)), session("older", minutesAgo(45))],
      NOW,
    );

    expect(groups).toEqual([
      {
        label: null,
        sessions: [session("newer", minutesAgo(5))],
      },
      {
        label: "30 minutes ago",
        sessions: [session("older", minutesAgo(45))],
      },
    ]);
  });

  test("omits empty groups without reordering sessions", () => {
    const groups = groupSessionsByTime(
      [session("newer", minutesAgo(35)), session("older", minutesAgo(50))],
      NOW,
    );

    expect(groups).toEqual([
      {
        label: "30 minutes ago",
        sessions: [
          session("newer", minutesAgo(35)),
          session("older", minutesAgo(50)),
        ],
      },
    ]);
  });
});
