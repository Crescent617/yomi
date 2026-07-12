import { formatElapsed } from "../../utils";
import { activateSession as stateActivateSession } from "../../session";

export function statusColor(status: string): string {
  switch (status) {
    case "running":
      return "text-warning border-warning/20 bg-warning/10";
    case "completed":
      return "text-success border-success/20 bg-success/10";
    case "failed":
      return "text-error border-error/20 bg-error/10";
    case "cancelled":
      return "text-muted-foreground border-muted-foreground/20 bg-muted-foreground/10";
    default:
      return "text-muted-foreground border-muted-foreground/20 bg-muted-foreground/10";
  }
}

export function compactArgs(args: string, maxLen = 120): string {
  if (!args) return "";
  try {
    const parsed = JSON.parse(args);
    const s = JSON.stringify(parsed);
    if (s.length <= maxLen) return s;
    return s.slice(0, maxLen) + "…";
  } catch {
    return (
      args.replace(/\s+/g, " ").slice(0, maxLen) +
      (args.length > maxLen ? "…" : "")
    );
  }
}

export function extractTarget(tool_name: string, args: string): string {
  if (!args) return "";
  try {
    const parsed = JSON.parse(args);
    switch (tool_name.toLowerCase()) {
      case "read":
      case "edit":
        return parsed.path ?? "";
      case "write":
        return parsed.file_path ?? "";
      case "shell":
        return parsed.command ?? "";
      case "glob":
      case "grep":
        return parsed.pattern ?? "";
      case "webfetch":
        return parsed.url ?? "";
      case "skill":
        return parsed.name ?? parsed.path ?? "";
      case "agent":
      case "subagent":
        return parsed.description ?? "";
      default:
        return "";
    }
  } catch {
    return "";
  }
}

export function extraMeta(tool_name: string, args: string): string {
  if (!args) return "";
  try {
    const parsed = JSON.parse(args);
    const extras: string[] = [];
    switch (tool_name.toLowerCase()) {
      case "shell": {
        if (parsed.background) extras.push("async");
        const timeout = parsed.timeout;
        if (timeout != null && (parsed.background || timeout !== 60)) {
          extras.push(`timeout ${timeout}s`);
        }
        break;
      }
      case "grep": {
        const mode = parsed.output_mode || "filename";
        if (mode !== "filename") extras.push(mode);
        break;
      }
      case "agent":
      case "subagent": {
        const preset = parsed.preset || "general-purpose";
        if (preset !== "general-purpose") extras.push(preset);
        break;
      }
    }
    return extras.join(" · ");
  } catch {
    return "";
  }
}

export interface EditArgs {
  path: string;
  old_str: string;
  new_str: string;
}

export interface WriteArgs {
  file_path: string;
  content: string;
}

export function parseEditArgs(args: string): EditArgs | null {
  try {
    const parsed = JSON.parse(args);
    if (
      typeof parsed.path === "string" &&
      typeof parsed.old_str === "string" &&
      typeof parsed.new_str === "string"
    ) {
      return parsed;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function parseWriteArgs(args: string): WriteArgs | null {
  try {
    const parsed = JSON.parse(args);
    if (
      typeof parsed.file_path === "string" &&
      typeof parsed.content === "string"
    ) {
      return parsed;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function diffLines(
  oldStr: string,
  newStr: string,
): { type: "add" | "del" | "context"; text: string }[] {
  const oldLines = oldStr.split("\n");
  const newLines = newStr.split("\n");

  const dp: number[][] = Array(oldLines.length + 1)
    .fill(null)
    .map(() => Array(newLines.length + 1).fill(0));

  for (let i = 1; i <= oldLines.length; i++) {
    for (let j = 1; j <= newLines.length; j++) {
      if (oldLines[i - 1] === newLines[j - 1]) {
        dp[i][j] = dp[i - 1][j - 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
      }
    }
  }

  const result: { type: "add" | "del" | "context"; text: string }[] = [];
  let i = oldLines.length;
  let j = newLines.length;

  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && oldLines[i - 1] === newLines[j - 1]) {
      result.push({ type: "context", text: oldLines[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      result.push({ type: "add", text: newLines[j - 1] });
      j--;
    } else {
      result.push({ type: "del", text: oldLines[i - 1] });
      i--;
    }
  }
  result.reverse();
  return result;
}

export { formatElapsed };

export async function handleJumpToSubagent(sessionId: string) {
  await stateActivateSession(sessionId);
}
