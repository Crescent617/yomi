/**
 * Attachment declarations in assistant texts (`<yomi_attachments>`).
 *
 * Port of `kernel::utils::attachments::parse_attachments` — keep the two
 * implementations (and their test cases) in sync. A block counts as a
 * declaration only when it stands outside a fenced code block (fence
 * parity tracked from the start of the text, exactly like Rust's
 * `map_outside_fences`: a marker line is one whose first non-whitespace
 * characters are ```; an unterminated fence keeps everything after the
 * opener fenced). Recognized blocks are stripped for display; stored
 * messages keep the raw text.
 */

const OPEN_TAG = "<yomi_attachments>";
const CLOSE_TAG = "</yomi_attachments>";

/**
 * Apply `f` to each contiguous run of `text` standing outside a fenced
 * code block; fenced runs (fence markers included) pass through verbatim.
 * Mirrors `kernel::utils::markdown::map_outside_fences`.
 */
function mapOutsideFences(text: string, f: (run: string) => string): string {
  let out = "";
  let fenced = false;
  let runStart = 0;
  let pos = 0;
  for (const line of text.split(/(?<=\n)/)) {
    const lineEnd = pos + line.length;
    if (line.trimStart().startsWith("```")) {
      if (!fenced) out += f(text.slice(runStart, pos));
      fenced = !fenced;
      out += line;
      pos = lineEnd;
      if (!fenced) runStart = pos;
    } else {
      pos = lineEnd;
      if (fenced) out += line;
    }
  }
  if (!fenced) out += f(text.slice(runStart, pos));
  return out;
}

export interface ParsedAttachments {
  cleaned: string;
  paths: string[];
}

/**
 * Strip every `<yomi_attachments>…</yomi_attachments>` block standing
 * outside a fenced code block, returning the cleaned text and the
 * declared paths (trimmed, non-empty, in document order). Fenced
 * examples and unterminated blocks are left in place.
 */
export function parseAttachments(text: string): ParsedAttachments {
  const paths: string[] = [];
  let removed = false;
  const cleaned = mapOutsideFences(text, (run) => {
    let out = "";
    let rest = run;
    for (;;) {
      const open = rest.indexOf(OPEN_TAG);
      if (open === -1) break;
      const afterOpen = open + OPEN_TAG.length;
      const close = rest.indexOf(CLOSE_TAG, afterOpen);
      if (close === -1) break;

      out += rest.slice(0, open);
      for (const line of rest.slice(afterOpen, close).split("\n")) {
        const trimmed = line.trim();
        if (trimmed) paths.push(trimmed);
      }
      removed = true;
      rest = rest.slice(close + CLOSE_TAG.length);
    }
    out += rest;
    return out;
  });
  if (!removed) return { cleaned: text, paths };
  return { cleaned: cleaned.trim(), paths };
}
