import type { Message } from "../../state.svelte";

export interface LoadedSkill {
  name: string;
  path: string;
}

// Matches any ".../skills/<name>/SKILL.md" read path — covers user skills
// (~/.agents/skills), data-dir skills (~/.yomi/skills), and workspace
// skills (<project>/.agents/skills).
const SKILL_PATH_RE = /\/skills\/([^/]+)\/SKILL\.md$/i;

function skillPathFromArguments(args: string | undefined): string | null {
  if (!args) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(args);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const path = (parsed as Record<string, unknown>).path;
  return typeof path === "string" && path ? path : null;
}

/**
 * Skills the session has loaded, derived from `read` tool calls on
 * SKILL.md files (yomi's skill-load convention). Historical semantics: a
 * skill stays listed even after compaction drops it from the agent's
 * context. Shell `cat`/grep reads are not tracked; subagent loads belong
 * to their own sessions. Deduped by skill name, first-seen order.
 */
export function loadedSkills(messages: Message[]): LoadedSkill[] {
  const byName = new Map<string, LoadedSkill>();
  for (const message of messages) {
    if (message.type !== "tool" || message.tool_name !== "read") continue;
    const path = skillPathFromArguments(message.arguments);
    if (!path) continue;
    const match = SKILL_PATH_RE.exec(path);
    if (!match) continue;
    const name = match[1];
    if (!byName.has(name)) byName.set(name, { name, path });
  }
  return [...byName.values()];
}
