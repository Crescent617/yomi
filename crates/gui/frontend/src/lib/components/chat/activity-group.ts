import type { Message } from "../../state.svelte";
import { findThinking } from "../../state.svelte";

export function isActivityTail(message: Message | undefined): boolean {
  if (!message) return false;
  if (message.type === "tool") return true;
  if (message.type !== "assistant") return false;
  if (message.tool_calls?.length) return true;

  const lastBlock = message.content.at(-1);
  if (lastBlock?.type === "text" && lastBlock.text.trim().length > 0) {
    return false;
  }

  return findThinking(message.content) !== null;
}
