import { describe, expect, test } from "vitest";
import type { SubagentInfo } from "../../api";
import {
  formatSubagentPhase,
  runningSubagents,
  runningSubagentsSummary,
} from "./running-subagents";

function subagent(partial: Partial<SubagentInfo>): SubagentInfo {
  return {
    id: "subagent",
    parent_session_id: "parent",
    alias: null,
    phase: "streaming",
    is_running: true,
    created_at: "2026-07-12T00:00:00Z",
    model_key: null,
    ...partial,
  };
}

describe("running subagents", () => {
  test("keeps todo-independent running state filtering", () => {
    expect(
      runningSubagents([
        subagent({ id: "running" }),
        subagent({ id: "finished", phase: "idle", is_running: false }),
      ]).map((item) => item.id),
    ).toEqual(["running"]);
  });

  test("summarizes one agent by description and multiple agents by count", () => {
    expect(runningSubagentsSummary([subagent({ alias: "Review tests" })])).toBe(
      "Review tests",
    );
    expect(
      runningSubagentsSummary([subagent({}), subagent({ id: "second" })]),
    ).toBe("2 agents");
  });

  test("formats snake_case phases for display", () => {
    expect(formatSubagentPhase("executing_tool")).toBe("executing tool");
  });
});
