/**
 * Attachment declarations in assistant texts (`<yomi_attachments>`).
 *
 * Port of `kernel::utils::attachments::parse_attachments` — keep the two
 * implementations (and their test cases) in sync. A block counts as a
 * declaration only when it stands outside a fenced code block (fence
 * parity: an odd fence count after the block means it is fenced in).
 * Recognized blocks are stripped for display; stored messages keep the
 * raw text.
 */

const OPEN_TAG = "<yomi_attachments>";
const CLOSE_TAG = "</yomi_attachments>";
const FENCE = "```";

export interface ParsedAttachments {
  cleaned: string;
  paths: string[];
}

function countOccurrences(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1;
}

/**
 * Strip every `<yomi_attachments>…</yomi_attachments>` block standing
 * outside a fenced code block, returning the cleaned text and the
 * declared paths (trimmed, non-empty, in document order). Fenced
 * examples and unterminated blocks are left in place.
 */
export function parseAttachments(text: string): ParsedAttachments {
  const paths: string[] = [];
  let cleaned = "";
  let removed = false;
  let rest = text;

  for (;;) {
    const open = rest.indexOf(OPEN_TAG);
    if (open === -1) break;
    const afterOpen = open + OPEN_TAG.length;
    const close = rest.indexOf(CLOSE_TAG, afterOpen);
    if (close === -1) break;
    const blockEnd = close + CLOSE_TAG.length;

    if (countOccurrences(rest.slice(blockEnd), FENCE) % 2 !== 0) {
      // Fenced example: keep it verbatim, keep scanning after it.
      cleaned += rest.slice(0, blockEnd);
      rest = rest.slice(blockEnd);
      continue;
    }

    cleaned += rest.slice(0, open);
    for (const line of rest.slice(afterOpen, close).split("\n")) {
      const trimmed = line.trim();
      if (trimmed) paths.push(trimmed);
    }
    removed = true;
    rest = rest.slice(blockEnd);
  }

  if (!removed) return { cleaned: text, paths };
  cleaned += rest;
  return { cleaned: cleaned.trim(), paths };
}
