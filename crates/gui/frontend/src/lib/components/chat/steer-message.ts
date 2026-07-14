export interface ParsedSteerMessage {
  agentId: string | null;
  content: string;
}

const AGENT_ID_PREFIX = /^\s*\[agent_id:\s*([^\]\r\n]+?)\s*\]\s*/i;

/** Extract the subagent session prefix emitted by the kernel. */
export function parseSteerMessage(content: string): ParsedSteerMessage {
  const match = content.match(AGENT_ID_PREFIX);
  if (!match) return { agentId: null, content };

  const agentId = match[1].trim();
  if (!agentId) return { agentId: null, content };

  return {
    agentId,
    content: content.slice(match[0].length),
  };
}
