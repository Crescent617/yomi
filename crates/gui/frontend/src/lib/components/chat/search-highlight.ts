/**
 * Find-in-chat highlight layer.
 *
 * Highlights ride the CSS Custom Highlight API (a Range registry, no DOM
 * mutation) so Svelte's streaming re-renders never fight wrapper
 * elements; stale ranges are simply dropped by the browser.
 *
 * Counting vs locating: findMatches counts with toLowerCase (full case
 * mapping), rangeForOccurrence locates with /iu (simple case folding).
 * They disagree only where a character EXPANDS under lowercasing
 * (İ→i̇): such a match is counted but not locatable, and degrades to
 * scroll-without-highlight — never to a stale range.
 */

const HIGHLIGHT_NAME = "yomi-search-active";

export function searchHighlightSupported(): boolean {
  return (
    typeof CSS !== "undefined" &&
    "highlights" in CSS &&
    typeof Highlight !== "undefined"
  );
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Range spanning the nth (0-based) case-insensitive occurrence of
 *  `query` across root's text nodes, or null when the rendered text
 *  holds fewer occurrences than the data layer counted (collapsed
 *  details, markdown re-flow, display-stripped markers).
 *
 *  The match may straddle element boundaries: rendering shreds words
 *  into multiple text nodes (syntax-highlight token spans, inline
 *  markup), and a single-node lookup would miss those occurrences and
 *  skew every later ordinal. Matching runs as a case-insensitive regex
 *  over the concatenated ORIGINAL text, so offsets always index real
 *  text — lowercasing a copy would shift offsets wherever a character
 *  expands under case folding (İ→i̇). */
export function rangeForOccurrence(
  root: Element,
  query: string,
  occurrence: number,
): Range | null {
  if (!query) return null;
  const walker = root.ownerDocument.createTreeWalker(
    root,
    NodeFilter.SHOW_TEXT,
  );
  const nodes: Text[] = [];
  let combined = "";
  let node = walker.nextNode() as Text | null;
  while (node) {
    nodes.push(node);
    combined += node.data;
    node = walker.nextNode() as Text | null;
  }

  const needle = new RegExp(escapeRegExp(query), "giu");
  let seen = 0;
  for (const hit of combined.matchAll(needle)) {
    if (seen !== occurrence) {
      seen += 1;
      continue;
    }
    const start = hit.index;
    const end = start + hit[0].length;
    // Map the combined-string offsets back to (node, offset) pairs.
    let offset = 0;
    let startNode: Text | null = null;
    let startOffset = 0;
    for (const candidate of nodes) {
      const nodeStart = offset;
      const nodeEnd = nodeStart + candidate.data.length;
      if (startNode === null && start <= nodeEnd) {
        startNode = candidate;
        startOffset = start - nodeStart;
      }
      if (end <= nodeEnd) {
        if (startNode === null) return null;
        try {
          const range = root.ownerDocument.createRange();
          range.setStart(startNode, startOffset);
          range.setEnd(candidate, end - nodeStart);
          return range;
        } catch {
          return null;
        }
      }
      offset = nodeEnd;
    }
    return null;
  }
  return null;
}

/** The caller clears before setting: a null range (occurrence not
 *  locatable in the rendered DOM) must leave NOTHING painted rather
 *  than the previous match's stale range. */
export function highlightOccurrence(
  root: Element,
  query: string,
  occurrence: number,
): boolean {
  if (!searchHighlightSupported()) return false;
  const range = rangeForOccurrence(root, query, occurrence);
  if (!range) return false;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (CSS as any).highlights.set(HIGHLIGHT_NAME, new Highlight(range));
  return true;
}

export function clearSearchHighlight(): void {
  if (!searchHighlightSupported()) return;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (CSS as any).highlights.delete(HIGHLIGHT_NAME);
}
