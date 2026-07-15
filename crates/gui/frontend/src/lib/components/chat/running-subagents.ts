import { isActiveSessionPhase } from "../../session-phase";
import type { SubagentInfo } from "../../api";

export function runningSubagents(subagents: SubagentInfo[]): SubagentInfo[] {
  return subagents.filter((subagent) => isActiveSessionPhase(subagent.phase));
}

export function subagentDescription(subagent: SubagentInfo): string {
  return subagent.alias?.trim() || "Agent";
}

export function runningSubagentsSummary(subagents: SubagentInfo[]): string {
  if (subagents.length === 1) return subagentDescription(subagents[0]);
  return `${subagents.length} agents`;
}

export function formatSubagentPhase(phase: string): string {
  return phase.replaceAll("_", " ");
}
