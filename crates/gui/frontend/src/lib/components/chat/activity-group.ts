import type { Message } from "../../state.svelte";
import { findThinking } from "../../session";

export function isAgentActivity(message: Message): boolean {
  return (
    message.type === "tool" &&
    (message.tool_name?.trim().toLowerCase() === "agent" ||
      Boolean(message.subagent_session_id))
  );
}

export function isActivityTail(message: Message | undefined): boolean {
  if (!message) return false;
  if (message.type === "tool") return true;
  if (message.type !== "assistant") return false;
  if (message.tool_calls?.length) return true;

  const lastBlock = message.content.at(-1);
  if (lastBlock?.type === "text" && lastBlock.text?.trim()) {
    return false;
  }

  return findThinking(message.content) !== null;
}
