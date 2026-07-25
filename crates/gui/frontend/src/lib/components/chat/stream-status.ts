// Token estimation for the inline stream status, mirroring the kernel's
// `utils::tokens` heuristics so both sides agree on the numbers:
// - plain text (thinking): 1 token ≈ 4 UTF-8 bytes
// - JSON (tool call arguments): 1 token ≈ 2 UTF-8 bytes (denser punctuation)
//
// These are estimates; the provider only reports exact usage at stream end.

import { extractTarget, normalizeToolName } from "../tool/tool-utils";

const encoder = new TextEncoder();

const utf8Length = (text: string): number => encoder.encode(text).length;

export function estimateTextTokens(text: string): number {
  return Math.ceil(utf8Length(text) / 4);
}

export function estimateJsonTokens(text: string): number {
  return Math.ceil(utf8Length(text) / 2);
}

// Display format mirroring the kernel's `format_estimated_tokens`:
// `~` prefix marks the value as an estimate.
export function formatStreamTokens(count: number): string {
  const word = count === 1 ? "token" : "tokens";
  if (count >= 1000) return `~${(count / 1000).toFixed(1)}k ${word}`;
  return `~${count} ${word}`;
}

/**
 * Present-tense verb for a tool while it runs, Claude Code style
 * ("Editing foo.ts" instead of "Calling Edit"). Unknown tools fall back
 * to "Calling" and keep the humanized tool name beside it.
 */
export function toolVerb(toolName: string): string {
  const verbs: Record<string, string> = {
    read: "Reading",
    edit: "Editing",
    write: "Writing",
    shell: "Running",
    glob: "Searching",
    grep: "Searching",
    webfetch: "Fetching",
    websearch: "Searching",
    agent: "Delegating",
    sleep: "Sleeping",
  };
  return verbs[normalizeToolName(toolName)] ?? "Calling";
}

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
  webfetch: ["url"],
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
export function formatRunElapsed(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  if (s < 60) return `${s}s`;
  const minutes = Math.floor(s / 60);
  const seconds = s % 60;
  if (minutes < 60) return `${minutes}m${seconds}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h${minutes % 60}m${seconds}s`;
}
