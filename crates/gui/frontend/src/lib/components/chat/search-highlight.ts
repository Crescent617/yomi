/**
 * Find-in-chat highlight layer.
 *
 * Highlights ride the CSS Custom Highlight API (a Range registry, no DOM
 * mutation) so Svelte's streaming re-renders never fight wrapper
 * elements; stale ranges are simply dropped by the browser. Where the
 * API is missing the caller falls back to flashing the message frame.
 */

const HIGHLIGHT_NAME = "yomi-search-active";

export function searchHighlightSupported(): boolean {
  return (
    typeof CSS !== "undefined" &&
    "highlights" in CSS &&
    typeof Highlight !== "undefined"
  );
}

/** Range spanning the nth (0-based) case-insensitive occurrence of
 *  `query` across root's text nodes, or null when the rendered text
 *  holds fewer occurrences than the data layer counted (collapsed
 *  details, markdown re-flow). */
export function rangeForOccurrence(
  root: Element,
  query: string,
  occurrence: number,
): Range | null {
  if (!query) return null;
  const needle = query.toLowerCase();
  const walker = root.ownerDocument.createTreeWalker(
    root,
    NodeFilter.SHOW_TEXT,
  );
  let node = walker.nextNode() as Text | null;
  let seen = 0;
  while (node) {
    const haystack = node.data.toLowerCase();
    let from = 0;
    while (from <= haystack.length - needle.length) {
      const hit = haystack.indexOf(needle, from);
      if (hit === -1) break;
      if (seen === occurrence) {
        try {
          const range = root.ownerDocument.createRange();
          range.setStart(node, hit);
          range.setEnd(node, hit + needle.length);
          return range;
        } catch {
          // 大小写折叠会扩长个别字符（İ→i̇），lowercased 串上的偏移
          // 可能越过原节点末尾 —— 降级为只滚动不高亮。
          return null;
        }
      }
      seen += 1;
      from = hit + needle.length;
    }
    node = walker.nextNode() as Text | null;
  }
  return null;
}

/** A match can straddle adjacent text nodes only when the query itself
 *  spans markup boundaries — single-node ranges cover the common case;
 *  the caller treats null as "scroll without highlight". */
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
