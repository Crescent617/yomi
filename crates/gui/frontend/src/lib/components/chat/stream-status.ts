// Token formatting and status-line helpers for the inline stream status.
// Byte-count estimation lives in `lib/tokens` (mirrors the kernel).

import { extractTarget, normalizeToolName } from "../tool/tool-utils";

// Display format mirroring the kernel's `format_estimated_tokens`:
// `~` prefix marks the value as an estimate.
export function formatStreamTokens(count: number): string {
  const word = count === 1 ? "token" : "tokens";
  if (count >= 1000) return `~${(count / 1000).toFixed(1)}k ${word}`;
  return `~${count} ${word}`;
}

/**
 * Present-tense verb for a tool while it runs, Claude Code style
 * ("Editing foo.ts" instead of "Calling Edit"). Shell commands get a
 * command-aware verb via `shellVerb`. Unknown tools fall back to
 * "Calling" and keep the humanized tool name beside it.
 */
export function toolVerb(toolName: string, rawArgs = ""): string {
  const name = normalizeToolName(toolName);
  if (name === "shell") return shellVerb(rawArgs);
  const verbs: Record<string, string> = {
    read: "Reading",
    edit: "Editing",
    write: "Writing",
    glob: "Finding",
    grep: "Finding",
    websearch: "Searching",
    agent: "Delegating",
    sleep: "Waiting",
    todo: "Planning",
    cron: "Scheduling",
    reminder: "Scheduling",
    askuser: "Asking",
    skill: "Invoking",
    postmessage: "Messaging",
    taskcreate: "Creating task",
    tasklist: "Listing tasks",
    taskget: "Reading task",
    taskupdate: "Updating task",
  };
  return verbs[name] ?? "Calling";
}

/**
 * Command-aware verb for shell calls: "Building" for cargo build, "Testing"
 * for npm test, ... Matches on the first command of the pipeline so
 * `cargo test && echo done` still reads as Testing. Falls back to "Running".
 * Tolerates args still streaming in (truncated JSON).
 */
export function shellVerb(rawArgs: string): string {
  const command = extractPartialTarget("shell", rawArgs) || rawArgs;
  const first = command.trim().split(/[|&;]/)[0].trim();
  for (const [pattern, verb] of COMMAND_VERBS) {
    if (pattern.test(first)) return verb;
  }
  return "Running";
}

const COMMAND_VERBS: Array<[RegExp, string]> = [
  [
    /^(?:sudo\s+)?(?:cargo\s+(?:build|check|clippy|fmt|doc)\b|npm\s+(?:run\s+(?:build|check|lint)|exec\s+\S*build)\b|pnpm\s+(?:run\s+)?build\b|yarn\s+build\b|make\b|cmake\b|go\s+build\b)/,
    "Building",
  ],
  [
    /^(?:sudo\s+)?(?:cargo\s+test\b|npm\s+(?:test|run\s+test)\b|pnpm\s+(?:test|run\s+test)\b|yarn\s+test\b|vitest\b|pytest\b|go\s+test\b|playwright\b)/,
    "Testing",
  ],
  [
    /^(?:npm|pnpm|yarn|bun)\s+(?:install|i|add)\b|^pip(?:3)?\s+install\b|^cargo\s+add\b|^brew\s+install\b/,
    "Installing",
  ],
  [/^git\s+commit\b/, "Committing"],
  [/^git\s+push\b/, "Pushing"],
  [/^git\s+(?:pull|fetch|rebase|merge)\b/, "Syncing"],
  [/^git\s+(?:checkout|switch)\b/, "Switching"],
  [/^git\s+(?:diff|log|status|show)\b/, "Inspecting"],
  [/^git\b/, "Running git"],
  [/^(?:rg|grep|findstr|ack|ag)\b/, "Searching"],
  [/^(?:find|fd|ls|tree|dir)\b/, "Exploring"],
  [/^(?:cat|head|tail|less|bat)\b/, "Reading"],
  [/^(?:rm|rmdir|del)\b/, "Removing"],
  [/^(?:mv|move|rename)\b/, "Moving"],
  [/^(?:cp|copy|xcopy|rsync)\b/, "Copying"],
  [/^(?:mkdir|touch)\b/, "Creating"],
  [/^(?:curl|wget)\b/, "Fetching"],
  [/^(?:ssh|scp|sftp)\b/, "Connecting"],
  [/^(?:kill|pkill|killall)\b/, "Killing"],
  [/^(?:tar|zip|unzip|gzip)\b/, "Archiving"],
  [/^(?:docker|podman|kubectl|helm)\b/, "Orchestrating"],
  [/^(?:python|python3|node|deno|ruby|perl)\b/, "Running script"],
  [/^(?:sed|awk|jq|yq)\b/, "Processing"],
  [/^(?:chmod|chown)\b/, "Permitting"],
  [/^sleep\b/, "Waiting"],
];

/** Arg keys carrying the display target, per normalized tool name.
 *  Only tools with a dedicated verb need an entry — "Calling" tools show
 *  their humanized name instead of a target. */
const TARGET_KEYS: Record<string, string[]> = {
  read: ["path"],
  edit: ["path"],
  write: ["file_path"],
  shell: ["command"],
  glob: ["pattern"],
  grep: ["pattern"],
  websearch: ["query"],
  agent: ["description", "prompt"],
};

// Matches `"key": "value"` pairs even in truncated JSON — the closing
// quote is optional so values still streaming in are picked up.
const ARG_PAIR_RE = /"([a-zA-Z_]+)"\s*:\s*"((?:[^"\\]|\\.)*)"?/g;

/**
 * Lenient target extraction for tool-call arguments that may still be
 * streaming (truncated JSON). Tries a strict parse via `extractTarget`
 * first, then falls back to scanning raw `"key": "value"` pairs in
 * argument order — the target key (path, command, ...) is sent first by
 * convention, so it is available from the earliest deltas.
 */
export function extractPartialTarget(
  toolName: string,
  rawArgs: string,
): string {
  if (!rawArgs) return "";
  const strict = extractTarget(toolName, rawArgs);
  if (strict) return strict;

  const keys = TARGET_KEYS[normalizeToolName(toolName)];
  if (!keys) return "";
  ARG_PAIR_RE.lastIndex = 0;
  for (
    let match = ARG_PAIR_RE.exec(rawArgs);
    match;
    match = ARG_PAIR_RE.exec(rawArgs)
  ) {
    if (keys.includes(match[1])) {
      return match[2].replace(/\s+/g, " ").trim();
    }
  }
  return "";
}

/** Elapsed run time for the status line: `8s`, `1m24s`, `1h2m33s`. */
export function formatTapeElapsed(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  if (s < 60) return `${s}s`;
  const minutes = Math.floor(s / 60);
  const seconds = s % 60;
  if (minutes < 60) return `${minutes}m${seconds}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h${minutes % 60}m${seconds}s`;
}
