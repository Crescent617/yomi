/**
 * Text helpers for the share card: markdown → plain text, and width-aware
 * line wrapping. Kept pure (no DOM/canvas) so they are unit-testable.
 */

/** Convert markdown source to readable plain text for the share card. */
export function markdownToPlainText(md: string): string {
  const lines = md.replace(/\r\n/g, "\n").split("\n");
  const out: string[] = [];
  let inFence = false;

  for (const line of lines) {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) {
      out.push(line);
      continue;
    }
    // Horizontal rules
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) continue;

    let text = line;
    // Headings
    text = text.replace(/^\s{0,3}#{1,6}\s+/, "");
    // Blockquotes
    text = text.replace(/^\s*>\s?/, "");
    // Images: keep alt text
    text = text.replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1");
    // Links: keep link text
    text = text.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
    // Bold / italic markers
    text = text.replace(/(\*\*|__)(.*?)\1/g, "$2");
    text = text.replace(/(\*|_)(.*?)\1/g, "$2");
    // Strikethrough
    text = text.replace(/~~(.*?)~~/g, "$1");
    // Inline code
    text = text.replace(/`([^`]*)`/g, "$1");
    // HTML tags
    text = text.replace(/<\/?[a-zA-Z][^>]*>/g, "");

    out.push(text);
  }

  return out
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/[ \t]+\n/g, "\n")
    .trim();
}

export type MeasureFn = (text: string) => number;

/**
 * Wrap text into lines that fit `maxWidth`. Splits on whitespace when
 * possible and falls back to breaking anywhere (CJK / long tokens).
 * Hard newlines in the input are preserved.
 */
export function wrapText(
  text: string,
  maxWidth: number,
  measure: MeasureFn,
): string[] {
  const lines: string[] = [];
  for (const paragraph of text.split("\n")) {
    if (paragraph === "") {
      lines.push("");
      continue;
    }
    let rest = paragraph;
    while (rest.length > 0) {
      if (measure(rest) <= maxWidth) {
        lines.push(rest);
        break;
      }
      // Binary-search the longest fitting prefix.
      let lo = 1;
      let hi = rest.length;
      while (lo < hi) {
        const mid = Math.ceil((lo + hi) / 2);
        if (measure(rest.slice(0, mid)) <= maxWidth) lo = mid;
        else hi = mid - 1;
      }
      let cut = lo;
      // Prefer breaking at the last whitespace within the fitting prefix.
      const spaceMatch = rest.slice(0, cut).match(/\s(?=\S*$)/);
      if (spaceMatch?.index) cut = spaceMatch.index;
      lines.push(rest.slice(0, cut).trimEnd());
      rest = rest.slice(cut).trimStart();
    }
  }
  return lines;
}
