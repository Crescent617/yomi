import { textFromBlocks } from "../../session";
import type { Message } from "../../state.svelte";

/** One match occurrence inside one message. */
export interface SearchMatch {
  message_id: string;
  /** 0-based ordinal of this occurrence within the message's text. */
  occurrence: number;
}

/** Per-message match count, in message order. */
export interface MessageMatchCount {
  message_id: string;
  count: number;
}

export function countMatches(text: string, query: string): number {
  if (!query) return 0;
  const haystack = text.toLowerCase();
  const needle = query.toLowerCase();
  let count = 0;
  let from = 0;
  while (from <= haystack.length - needle.length) {
    const hit = haystack.indexOf(needle, from);
    if (hit === -1) break;
    count += 1;
    from = hit + needle.length; // non-overlapping, like browser find
  }
  return count;
}

/** Flat match list across messages, in display order. */
export function findMatches(messages: Message[], query: string): SearchMatch[] {
  if (!query.trim()) return [];
  const matches: SearchMatch[] = [];
  for (const message of messages) {
    // Tool/system entries carry no user-visible prose to search.
    if (message.type !== "user" && message.type !== "assistant") continue;
    const count = countMatches(textFromBlocks(message.content), query);
    for (let occurrence = 0; occurrence < count; occurrence++) {
      matches.push({ message_id: message.id, occurrence });
    }
  }
  return matches;
}

/** Clamp a (possibly stale) active index into the current match count. */
export function clampActiveIndex(index: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(Math.max(index, 0), total - 1);
}

/** Wrap-around step for next/previous navigation. */
export function stepActiveIndex(
  index: number,
  total: number,
  delta: 1 | -1,
): number {
  if (total <= 0) return 0;
  return (index + delta + total) % total;
}
