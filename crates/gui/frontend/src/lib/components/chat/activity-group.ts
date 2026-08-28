import type { Message } from "../../state.svelte";
import { findThinking } from "../../session";

export function isAgentActivity(message: Message): boolean {
  return (
    message.type === "tool" && categorizeToolName(message.tool_name) === "agent"
  );
}

const SEARCH_READ_TOOLS = new Set([
  "read",
  "readfile",
  "grep",
  "grepsearch",
  "glob",
  "globsearch",
  "websearch",
]);

export type ToolCategory =
  | "agent"
  | "editWrite"
  | "shell"
  | "searchRead"
  | "other";

/** Classify a tool name into an activity-badge category. Normalizes
 *  separators so both snake_case and legacy camelCase names match. */
export function categorizeToolName(rawName: string): ToolCategory {
  const name = rawName.trim().toLowerCase().replace(/[_-]/g, "");
  if (name === "agent") return "agent";
  if (["write", "writefile", "edit", "editfile"].includes(name))
    return "editWrite";
  if (["shell", "bash", "command"].includes(name)) return "shell";
  if (SEARCH_READ_TOOLS.has(name)) return "searchRead";
  return "other";
}

export interface ActivityStats {
  thinkingCount: number;
  searchReadCount: number;
  editWriteCount: number;
  shellCount: number;
  subagentCount: number;
  otherToolCount: number;
  failedCount: number;
  elapsedMs: number;
  actionCount: number;
}

export function computeActivityStats(messages: Message[]): ActivityStats {
  const stats: ActivityStats = {
    thinkingCount: 0,
    searchReadCount: 0,
    editWriteCount: 0,
    shellCount: 0,
    subagentCount: 0,
    otherToolCount: 0,
    failedCount: 0,
    elapsedMs: 0,
    actionCount: 0,
  };
  const bump = (category: ToolCategory) => {
    if (category === "agent") stats.subagentCount += 1;
    else if (category === "editWrite") stats.editWriteCount += 1;
    else if (category === "shell") stats.shellCount += 1;
    else if (category === "searchRead") stats.searchReadCount += 1;
    else stats.otherToolCount += 1;
  };

  for (const message of messages) {
    if (message.type === "assistant") {
      const thinking = findThinking(message.content);
      if (thinking) {
        stats.thinkingCount += 1;
        stats.elapsedMs += thinking.elapsed_ms ?? 0;
      }
      continue;
    }
    if (message.type !== "tool") continue;
    stats.elapsedMs += message.elapsed_ms ?? 0;
    if (message.status === "failed") stats.failedCount += 1;
    bump(categorizeToolName(message.tool_name));
  }

  stats.actionCount =
    stats.thinkingCount +
    stats.searchReadCount +
    stats.editWriteCount +
    stats.shellCount +
    stats.subagentCount +
    stats.otherToolCount;
  return stats;
}

export type ActivityTrailItem =
  | { type: "thought"; id: string; content: string; elapsed_ms: number }
  | { type: "tool"; id: string; message: Extract<Message, { type: "tool" }> };

/** Build the expanded activity trail from thinking blocks and tool messages. */
export function buildActivityTrail(messages: Message[]): ActivityTrailItem[] {
  const items: ActivityTrailItem[] = [];
  for (const message of messages) {
    if (message.type === "assistant") {
      const thinking = findThinking(message.content);
      if (thinking) {
        items.push({
          type: "thought",
          id: message.id,
          content: thinking.content,
          elapsed_ms: thinking.elapsed_ms,
        });
      }
    } else if (message.type === "tool") {
      items.push({ type: "tool", id: message.id, message });
    }
  }
  return items;
}
