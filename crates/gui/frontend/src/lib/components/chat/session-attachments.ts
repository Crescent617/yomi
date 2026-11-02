import { parseAttachments } from "../../attachments";
import { textFromBlocks } from "../../session";
import type { Message } from "../../state.svelte";

/**
 * Files the session has delivered, derived from `<yomi_attachments>`
 * declarations in assistant messages — the same blocks AttachmentChips
 * renders inline. Historical semantics: a file stays listed even after
 * compaction drops the declaring message from context. Deduped by path,
 * first-seen order. Fenced examples and unterminated blocks are not
 * declarations (parseAttachments skips them).
 */
export function sessionAttachments(messages: Message[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const message of messages) {
    if (message.type !== "assistant") continue;
    const { paths } = parseAttachments(textFromBlocks(message.content));
    for (const path of paths) {
      if (seen.has(path)) continue;
      seen.add(path);
      out.push(path);
    }
  }
  return out;
}
