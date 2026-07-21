/**
 * Lightweight markdown parser for the share card. Produces block-level
 * structure with styled inline runs — enough fidelity for headings, lists,
 * quotes, code and common inline emphasis without a full CommonMark stack.
 * Pure (no DOM/canvas) so it is unit-testable.
 */

export interface InlineStyle {
  bold?: boolean;
  italic?: boolean;
  code?: boolean;
  link?: boolean;
  strike?: boolean;
}

export interface InlineRun {
  text: string;
  style: InlineStyle;
}

export interface ListItem {
  /** Rendered marker, e.g. "•" or "3." */
  label: string;
  runs: InlineRun[];
}

export type TableAlign = "left" | "center" | "right";

export type Block =
  | { kind: "heading"; level: 1 | 2 | 3; runs: InlineRun[] }
  | { kind: "paragraph"; runs: InlineRun[] }
  | { kind: "list"; ordered: boolean; items: ListItem[] }
  | { kind: "quote"; runs: InlineRun[] }
  | { kind: "code"; text: string }
  | {
      kind: "table";
      header: InlineRun[][];
      align: TableAlign[];
      rows: InlineRun[][][];
    }
  | { kind: "hr" };

/** CJK / full-width ranges: used for line-break and join heuristics. */
const CJK_RE = /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef]/;
const CJK_END_RE = /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef]$/;
const CJK_START_RE = /^[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef]/;

export function isCjk(ch: string): boolean {
  return CJK_RE.test(ch);
}

const FENCE_RE = /^\s*(`{3,}|~{3,})/;
const HEADING_RE = /^\s{0,3}(#{1,6})\s+(.*)$/;
const HR_RE = /^\s*([-*_])(\s*\1){2,}\s*$/;
const QUOTE_RE = /^\s{0,3}>\s?(.*)$/;
const ULIST_RE = /^\s{0,3}[-*+]\s+(.*)$/;
const OLIST_RE = /^\s{0,3}(\d{1,9})[.)]\s+(.*)$/;
const TABLE_ROW_RE = /^\s*\|/;
const TABLE_SEP_RE = /^\s*\|?[\s\-:|]+\|?\s*$/;
const HTML_TAG_RE = /^<\/?[a-zA-Z][^>]*>/;

/** Join soft-wrapped source lines: no extra space between CJK characters. */
function joinLines(a: string, b: string): string {
  if (!a) return b;
  if (!b) return a;
  return CJK_END_RE.test(a) || CJK_START_RE.test(b) ? a + b : `${a} ${b}`;
}

export function parseMarkdown(md: string): Block[] {
  const lines = md.replace(/\r\n/g, "\n").split("\n");
  const blocks: Block[] = [];
  let para = "";

  const flushParagraph = () => {
    const text = para.trim();
    para = "";
    if (text) blocks.push({ kind: "paragraph", runs: parseInline(text) });
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block
    const fence = FENCE_RE.exec(line);
    if (fence) {
      flushParagraph();
      const ch = fence[1][0];
      const closeRE = new RegExp(`^\\s*${ch}{3,}`);
      const buf: string[] = [];
      i++;
      while (i < lines.length && !closeRE.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      i++; // skip closing fence (or EOF on unterminated fence)
      while (buf.length > 0 && buf[buf.length - 1].trim() === "") buf.pop();
      blocks.push({ kind: "code", text: buf.join("\n") });
      continue;
    }

    if (line.trim() === "") {
      flushParagraph();
      i++;
      continue;
    }

    const heading = HEADING_RE.exec(line);
    if (heading) {
      flushParagraph();
      const level = Math.min(3, heading[1].length) as 1 | 2 | 3;
      blocks.push({
        kind: "heading",
        level,
        runs: parseInline(heading[2].trim()),
      });
      i++;
      continue;
    }

    if (HR_RE.test(line)) {
      flushParagraph();
      blocks.push({ kind: "hr" });
      i++;
      continue;
    }

    const quote = QUOTE_RE.exec(line);
    if (quote) {
      flushParagraph();
      let text = quote[1];
      i++;
      while (i < lines.length) {
        const more = QUOTE_RE.exec(lines[i]);
        if (!more) break;
        text = joinLines(text, more[1].trim());
        i++;
      }
      const trimmed = text.trim();
      if (trimmed) blocks.push({ kind: "quote", runs: parseInline(trimmed) });
      continue;
    }

    const olist = OLIST_RE.exec(line);
    if (olist) {
      flushParagraph();
      const items: ListItem[] = [];
      while (i < lines.length) {
        const m = OLIST_RE.exec(lines[i]);
        if (!m) break;
        items.push({ label: `${m[1]}.`, runs: parseInline(m[2].trim()) });
        i++;
      }
      blocks.push({ kind: "list", ordered: true, items });
      continue;
    }

    const ulist = ULIST_RE.exec(line);
    if (ulist) {
      flushParagraph();
      const items: ListItem[] = [];
      while (i < lines.length) {
        const m = ULIST_RE.exec(lines[i]);
        if (!m) break;
        items.push({ label: "•", runs: parseInline(m[1].trim()) });
        i++;
      }
      blocks.push({ kind: "list", ordered: false, items });
      continue;
    }

    // GFM table: header row + separator row, then body rows.
    if (
      TABLE_ROW_RE.test(line) &&
      i + 1 < lines.length &&
      isSeparatorRow(lines[i + 1])
    ) {
      flushParagraph();
      const headerCells = splitRow(line);
      const alignCells = splitRow(lines[i + 1]);
      const cols = headerCells.length;
      const norm = (cells: string[]): string[] =>
        Array.from({ length: cols }, (_, k) => cells[k] ?? "");
      i += 2;
      const rows: InlineRun[][][] = [];
      while (i < lines.length && TABLE_ROW_RE.test(lines[i])) {
        if (!isSeparatorRow(lines[i])) {
          rows.push(norm(splitRow(lines[i])).map((cell) => parseInline(cell)));
        }
        i++;
      }
      blocks.push({
        kind: "table",
        header: norm(headerCells).map((cell) => parseInline(cell)),
        align: Array.from({ length: cols }, (_, k) =>
          parseAlign(alignCells[k] ?? ""),
        ),
        rows,
      });
      continue;
    }

    para = joinLines(para, line.trim());
    i++;
  }
  flushParagraph();
  return blocks;
}

/** Split a table row into trimmed cell texts (escaped pipes unsupported). */
function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|")) s = s.slice(0, -1);
  return s.split("|").map((c) => c.trim());
}

function parseAlign(cell: string): TableAlign {
  const left = cell.startsWith(":");
  const right = cell.endsWith(":");
  return left && right ? "center" : right ? "right" : "left";
}

/** Separator rows look like `|---|---|` with at least one dash per cell. */
function isSeparatorRow(line: string): boolean {
  return (
    TABLE_SEP_RE.test(line) && splitRow(line).every((c) => c.includes("-"))
  );
}

// ── Inline parsing ──

export function parseInline(
  text: string,
  style: InlineStyle = {},
): InlineRun[] {
  const runs: InlineRun[] = [];
  let buf = "";
  const flush = () => {
    if (buf) {
      runs.push({ text: buf, style });
      buf = "";
    }
  };

  let i = 0;
  while (i < text.length) {
    const ch = text[i];

    // Backslash escapes
    if (
      ch === "\\" &&
      i + 1 < text.length &&
      /[*_~`[\]\\<>]/.test(text[i + 1])
    ) {
      buf += text[i + 1];
      i += 2;
      continue;
    }

    // Inline code
    if (ch === "`") {
      const end = text.indexOf("`", i + 1);
      if (end > i + 1) {
        flush();
        runs.push({
          text: text.slice(i + 1, end),
          style: { ...style, code: true },
        });
        i = end + 1;
        continue;
      }
    }

    // Image: keep alt text
    if (ch === "!" && text[i + 1] === "[") {
      const bracket = parseBracket(text, i + 1);
      if (bracket) {
        flush();
        if (bracket.label) runs.push(...parseInline(bracket.label, style));
        i = bracket.end;
        continue;
      }
    }

    // Link
    if (ch === "[") {
      const bracket = parseBracket(text, i);
      if (bracket && bracket.label) {
        flush();
        runs.push(...parseInline(bracket.label, { ...style, link: true }));
        i = bracket.end;
        continue;
      }
    }

    // Bold + italic (*** / ___)
    if (
      (ch === "*" || ch === "_") &&
      text[i + 1] === ch &&
      text[i + 2] === ch
    ) {
      if (flankingOk(text, i, i + 3, ch)) {
        const end = findClosing(text, ch + ch + ch, i + 3);
        if (end > i + 3) {
          flush();
          runs.push(
            ...parseInline(text.slice(i + 3, end), {
              ...style,
              bold: true,
              italic: true,
            }),
          );
          i = end + 3;
          continue;
        }
      }
    }

    // Bold (** / __)
    if ((ch === "*" || ch === "_") && text[i + 1] === ch) {
      if (flankingOk(text, i, i + 2, ch)) {
        const end = findClosing(text, ch + ch, i + 2);
        if (end > i + 2) {
          flush();
          runs.push(
            ...parseInline(text.slice(i + 2, end), { ...style, bold: true }),
          );
          i = end + 2;
          continue;
        }
      }
    }

    // Italic (* / _)
    if (ch === "*" || ch === "_") {
      if (flankingOk(text, i, i + 1, ch)) {
        const end = findClosingSingle(text, ch, i + 1);
        if (end > i + 1) {
          flush();
          runs.push(
            ...parseInline(text.slice(i + 1, end), { ...style, italic: true }),
          );
          i = end + 1;
          continue;
        }
      }
    }

    // Strikethrough
    if (ch === "~" && text[i + 1] === "~") {
      const end = findClosing(text, "~~", i + 2);
      if (end > i + 2) {
        flush();
        runs.push(
          ...parseInline(text.slice(i + 2, end), { ...style, strike: true }),
        );
        i = end + 2;
        continue;
      }
    }

    // HTML tags: drop
    if (ch === "<") {
      const tag = HTML_TAG_RE.exec(text.slice(i));
      if (tag) {
        i += tag[0].length;
        continue;
      }
    }

    buf += ch;
    i++;
  }
  flush();
  return mergeRuns(runs);
}

/** `[label](url)` starting at `start` (which points at "["). */
function parseBracket(
  text: string,
  start: number,
): { label: string; end: number } | null {
  const close = text.indexOf("](", start + 1);
  if (close === -1) return null;
  const paren = text.indexOf(")", close + 2);
  if (paren === -1) return null;
  return { label: text.slice(start + 1, close), end: paren + 1 };
}

/**
 * Opening-marker guard: content must not start with whitespace, and `_`
 * markers inside words (snake_case) are literal.
 */
function flankingOk(
  text: string,
  start: number,
  contentStart: number,
  ch: string,
): boolean {
  const next = text[contentStart];
  if (!next || /\s/.test(next)) return false;
  if (ch === "_") {
    const prev = start > 0 ? text[start - 1] : "";
    if (prev && /[A-Za-z0-9_]/.test(prev)) return false;
  }
  return true;
}

function findClosing(text: string, mark: string, from: number): number {
  let idx = from;
  while ((idx = text.indexOf(mark, idx)) !== -1) {
    if (text[idx - 1] === "\\") {
      idx += mark.length;
      continue;
    }
    return idx;
  }
  return -1;
}

/** Find a single-char closing marker, skipping escaped chars and doubles. */
function findClosingSingle(text: string, ch: string, from: number): number {
  for (let i = from; i < text.length; i++) {
    if (text[i] !== ch) continue;
    if (text[i - 1] === "\\") continue;
    if (text[i + 1] === ch) {
      i++; // part of a double marker
      continue;
    }
    return i;
  }
  return -1;
}

function sameStyle(a: InlineStyle, b: InlineStyle): boolean {
  return (
    !!a.bold === !!b.bold &&
    !!a.italic === !!b.italic &&
    !!a.code === !!b.code &&
    !!a.link === !!b.link &&
    !!a.strike === !!b.strike
  );
}

/** Merge adjacent runs sharing the same style. */
function mergeRuns(runs: InlineRun[]): InlineRun[] {
  const out: InlineRun[] = [];
  for (const run of runs) {
    const last = out[out.length - 1];
    if (last && sameStyle(last.style, run.style)) {
      last.text += run.text;
    } else {
      out.push({ text: run.text, style: run.style });
    }
  }
  return out;
}
