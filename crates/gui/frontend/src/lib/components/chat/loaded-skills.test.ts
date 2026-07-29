import { describe, expect, test } from "vitest";
import { loadedSkills } from "./loaded-skills";
import type { ToolMessage } from "../../state.svelte";

let seq = 0;

function toolMessage(
  toolName: string,
  args: string,
  overrides?: Partial<ToolMessage>,
): ToolMessage {
  seq += 1;
  return {
    id: `t${seq}`,
    created_at: "2026-07-29T00:00:00Z",
    type: "tool",
    tool_call_id: `c${seq}`,
    tool_name: toolName,
    status: "completed",
    arguments: args,
    result: [],
    ...overrides,
  };
}

function readOn(path: string): ToolMessage {
  return toolMessage("read", JSON.stringify({ path }));
}

describe("loadedSkills", () => {
  test("extracts skills from user, data-dir, and workspace skill paths", () => {
    const skills = loadedSkills([
      readOn("~/.agents/skills/agent-browser/SKILL.md"),
      readOn("/Users/u/.yomi/skills/promql/SKILL.md"),
      readOn("/repo/.agents/skills/team-flow/SKILL.md"),
    ]);
    expect(skills.map((s) => s.name)).toEqual([
      "agent-browser",
      "promql",
      "team-flow",
    ]);
  });

  test("dedupes by name and keeps the first path", () => {
    const skills = loadedSkills([
      readOn("~/.agents/skills/promql/SKILL.md"),
      readOn("/repo/.agents/skills/promql/SKILL.md"),
    ]);
    expect(skills).toEqual([
      { name: "promql", path: "~/.agents/skills/promql/SKILL.md" },
    ]);
  });

  test("ignores non-read tools even when they cat a SKILL.md", () => {
    const skills = loadedSkills([
      toolMessage(
        "shell",
        JSON.stringify({ command: "cat ~/.agents/skills/promql/SKILL.md" }),
      ),
    ]);
    expect(skills).toEqual([]);
  });

  test("ignores reads of non-skill files", () => {
    const skills = loadedSkills([
      readOn("/repo/crates/kernel/src/main.rs"),
      readOn("/repo/docs/SKILL.md"),
    ]);
    expect(skills).toEqual([]);
  });

  test("ignores invalid arguments and missing paths", () => {
    const skills = loadedSkills([
      toolMessage("read", "not json"),
      toolMessage("read", "{}"),
      toolMessage("read", JSON.stringify({ path: 42 })),
      toolMessage("read", ""),
    ]);
    expect(skills).toEqual([]);
  });

  test("matches SKILL.md case-insensitively but not other .md files", () => {
    const skills = loadedSkills([
      readOn("~/.agents/skills/promql/Skill.md"),
      readOn("~/.agents/skills/promql/README.md"),
    ]);
    expect(skills.map((s) => s.name)).toEqual(["promql"]);
  });

  test("ignores non-tool messages", () => {
    const skills = loadedSkills([
      {
        id: "u1",
        created_at: "2026-07-29T00:00:00Z",
        type: "user",
        content: [],
      },
    ]);
    expect(skills).toEqual([]);
  });
});
