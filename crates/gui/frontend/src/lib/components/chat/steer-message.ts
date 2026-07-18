export type SteerSource =
  | { type: "user" }
  | { type: "agent"; id: string }
  | { type: "shell"; id: string };

export interface ParsedSteerMessage {
  source: SteerSource | null;
  content: string;
}

const FROM_USER_PREFIX = /^\s*\[from user\]\s*/i;
const FROM_AGENT_PREFIX = /^\s*\[from agent:\s*([^\]\r\n]+?)\s*\]\s*/i;
const FROM_SHELL_PREFIX = /^\s*\[from shell:\s*([^\]\r\n]+?)\s*\]\s*/i;
const LEGACY_AGENT_ID_PREFIX = /^\s*\[agent_id:\s*([^\]\r\n]+?)\s*\]\s*/i;

function parsePrefix(
  content: string,
  pattern: RegExp,
  type: "agent" | "shell",
): ParsedSteerMessage | null {
  const match = content.match(pattern);
  if (!match) return null;

  const id = match[1].trim();
  if (!id) return null;

  return {
    source: { type, id },
    content: content.slice(match[0].length),
  };
}

function parseUserPrefix(content: string): ParsedSteerMessage | null {
  const match = content.match(FROM_USER_PREFIX);
  if (!match) return null;

  return {
    source: { type: "user" },
    content: content.slice(match[0].length),
  };
}

/** Extract the source prefix emitted by user input, background tasks, and agents. */
export function parseSteerMessage(content: string): ParsedSteerMessage {
  return (
    parseUserPrefix(content) ??
    parsePrefix(content, FROM_AGENT_PREFIX, "agent") ??
    parsePrefix(content, FROM_SHELL_PREFIX, "shell") ??
    parsePrefix(content, LEGACY_AGENT_ID_PREFIX, "agent") ?? {
      source: null,
      content,
    }
  );
}
