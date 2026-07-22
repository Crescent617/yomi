/**
 * Escape intra-word underscore runs (`a_b`, `x__y`) so the
 * streaming-markdown renderer doesn't mistake them for italic delimiters.
 *
 * streaming-markdown doesn't implement CommonMark flanking rules: any `_`
 * followed by a non-space opens emphasis, and an unmatched one italicizes
 * the rest of the paragraph — e.g. a snake_case identifier like
 * `finish_reason`. Escaping intra-word runs (`\_`) renders them literally
 * while leaving real emphasis (`_foo_`, `__bold__`) intact.
 *
 * Fenced code, inline code spans, and link destinations are copied
 * verbatim so escaping can't corrupt code or URLs.
 */
export function escapeIntrawordUnderscores(md: string): string {
  let inFence = false;
  let fenceChar = "";
  return md
    .split("\n")
    .map((line) => {
      const fence = /^ {0,3}(```+|~~~+)/.exec(line);
      if (fence) {
        const marker = fence[1][0];
        if (!inFence) {
          inFence = true;
          fenceChar = marker;
        } else if (marker === fenceChar) {
          inFence = false;
        }
        return line;
      }
      return inFence ? line : escapeLine(line);
    })
    .join("\n");
}

function escapeLine(line: string): string {
  let out = "";
  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    // Inline code span: copy through the matching backtick run (or EOL).
    if (ch === "`") {
      const ticks = "`".repeat(countRun(line, i, "`"));
      const close = line.indexOf(ticks, i + ticks.length);
      const end = close === -1 ? line.length : close + ticks.length;
      out += line.slice(i, end);
      i = end;
      continue;
    }
    // Link destination `](url)`: copy through the closing paren.
    if (ch === "]" && line[i + 1] === "(") {
      const close = line.indexOf(")", i + 2);
      const end = close === -1 ? line.length : close + 1;
      out += line.slice(i, end);
      i = end;
      continue;
    }
    // Underscore run: escape the whole run when it sits inside a word.
    if (ch === "_") {
      const run = countRun(line, i, "_");
      out +=
        isAlnum(line[i - 1]) && isAlnum(line[i + run])
          ? "\\_".repeat(run)
          : "_".repeat(run);
      i += run;
      continue;
    }
    out += ch;
    i += 1;
  }
  return out;
}

function countRun(s: string, start: number, ch: string): number {
  let end = start;
  while (end < s.length && s[end] === ch) end += 1;
  return end - start;
}

function isAlnum(ch: string | undefined): boolean {
  return (
    ch !== undefined &&
    ((ch >= "0" && ch <= "9") ||
      (ch >= "a" && ch <= "z") ||
      (ch >= "A" && ch <= "Z"))
  );
}
