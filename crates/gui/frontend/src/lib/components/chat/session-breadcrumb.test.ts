import { describe, expect, test } from "vitest";
import { createSessionState } from "../../session";
import { buildSessionBreadcrumb } from "./session-breadcrumb";

describe("session breadcrumb", () => {
  test("builds the parent-to-subagent chain from the target session", () => {
    const parent = createSessionState({
      id: "session-parent",
      alias: "Parent",
    });
    const child = createSessionState({
      id: "sub-child",
      alias: "Child",
      parent_session_id: parent.id,
    });
    const sessions = new Map([
      [parent.id, parent],
      [child.id, child],
    ]);

    expect(buildSessionBreadcrumb(child, (id) => sessions.get(id))).toEqual([
      { id: parent.id, label: "Parent", isSubagent: false },
      { id: child.id, label: "Child", isSubagent: true },
    ]);
  });

  test("switching back to the parent does not retain a stale child crumb", () => {
    const parent = createSessionState({
      id: "session-parent",
      alias: "Parent",
    });
    const child = createSessionState({
      id: "sub-child",
      alias: "Child",
      parent_session_id: parent.id,
    });
    const sessions = new Map([
      [parent.id, parent],
      [child.id, child],
    ]);

    expect(buildSessionBreadcrumb(parent, (id) => sessions.get(id))).toEqual([
      { id: parent.id, label: "Parent", isSubagent: false },
    ]);
  });

  test("keeps an unloaded parent navigable", () => {
    const child = createSessionState({
      id: "sub-child",
      alias: "Child",
      parent_session_id: "session-parent",
    });

    expect(buildSessionBreadcrumb(child, () => undefined)).toEqual([
      { id: "session-parent", label: "…", isSubagent: false },
      { id: child.id, label: "Child", isSubagent: true },
    ]);
  });
});
