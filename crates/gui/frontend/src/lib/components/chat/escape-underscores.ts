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
 * Fenced code, inline code spans, link destinations, and bare URLs are
 * copied verbatim so escaping can't corrupt code or URLs.
 */
export function escapeIntrawordUnderscores(md: string): string {
  const state: FenceState = { inFence: false, fenceChar: "" };
  return md
    .split("\n")
    .map((line) => escapeLineStateful(line, state))
    .join("\n");
}

interface FenceState {
  inFence: boolean;
  fenceChar: string;
}

/**
 * Cap on the uncommitted tail line re-escaped on every update (~8 KiB).
 * Above it the tail passes through verbatim — see the class doc.
 */
const MAX_UNCOMMITTED_TAIL_CHARS = 8 * 1024;

/**
 * Incremental variant for streaming: caches the escaped output of every
 * newline-terminated line and only reprocesses the trailing partial line
 * (plus newly completed lines) on each update. Escaping is line-local —
 * inline code spans, link destinations, and bare URLs never extend past a
 * newline — so a line reprocessed with the fence state of its start yields
 * exactly the one-shot result. Falls back to a full pass when the input is
 * not an append (message switch, rewrite, retroactive details split).
 *
 * One exception to the per-update tail reprocessing: an uncommitted tail
 * longer than `MAX_UNCOMMITTED_TAIL_CHARS` is passed through verbatim.
 * Re-escaping a giant single-line stream (base64 data URL, huge link) in
 * full on every append degenerates to O(n²); the underscores in such a
 * tail stay unescaped until the line completes and is committed through
 * the normal path.
 */
export class IncrementalUnderscoreEscape {
  private source = "";
  private consumed = 0;
  private escaped = "";
  private inFence = false;
  private fenceChar = "";

  update(md: string): string {
    // When startsWith holds, md.length >= this.source.length >=
    // this.consumed, so the reset below is the only guard needed.
    if (!md.startsWith(this.source)) {
      this.consumed = 0;
      this.escaped = "";
      this.inFence = false;
      this.fenceChar = "";
    }
    this.source = md;

    // Commit newly completed lines; their fence state is final.
    const lastNewline = md.lastIndexOf("\n");
    if (lastNewline + 1 > this.consumed) {
      const lines = md.slice(this.consumed, lastNewline + 1).split("\n");
      lines.pop(); // trailing "" after the final newline
      const state: FenceState = {
        inFence: this.inFence,
        fenceChar: this.fenceChar,
      };
      const out: string[] = [];
      for (const line of lines) out.push(escapeLineStateful(line, state));
      this.escaped += out.join("\n") + "\n";
      this.inFence = state.inFence;
      this.fenceChar = state.fenceChar;
      this.consumed = lastNewline + 1;
    }

    // An over-long uncommitted tail (base64 data URL, huge link) would be
    // re-escaped in full on every append — O(n²) for the stream. Past the
    // cap, copy it verbatim; its fence status is already decided by its
    // fixed prefix, and it is escaped once when the line completes and is
    // committed above.
    if (md.length - this.consumed > MAX_UNCOMMITTED_TAIL_CHARS) {
      return this.escaped + md.slice(this.consumed);
    }

    // The trailing partial line is reprocessed on every update without
    // committing its fence state — it may still grow into a fence marker.
    const tailState: FenceState = {
      inFence: this.inFence,
      fenceChar: this.fenceChar,
    };
    return (
      this.escaped + escapeLineStateful(md.slice(this.consumed), tailState)
    );
  }
}

function escapeLineStateful(line: string, state: FenceState): string {
  const fence = /^ {0,3}(```+|~~~+)/.exec(line);
  if (fence) {
    const marker = fence[1][0];
    if (!state.inFence) {
      state.inFence = true;
      state.fenceChar = marker;
    } else if (marker === state.fenceChar) {
      state.inFence = false;
    }
    return line;
  }
  // escapeLine only rewrites underscore runs — skip lines without any.
  if (state.inFence || !line.includes("_")) return line;
  return escapeLine(line);
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
    // Bare URL (what the renderer autolinks): copy through the terminator —
    // escaping an underscore here inserts a backslash, which the renderer
    // treats as the end of the URL, truncating the link.
    if (line.startsWith("http://", i) || line.startsWith("https://", i)) {
      let end = i;
      while (end < line.length && !/[\s\\<>"'`]/.test(line[end])) end += 1;
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
  // Unicode letters and digits (covers ASCII snake_case and CJK words).
  return ch !== undefined && /[\p{L}\p{N}]/u.test(ch);
}
