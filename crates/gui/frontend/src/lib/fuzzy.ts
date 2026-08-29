/**
 * Subsequence fuzzy match for the command palette (VSCode quick-open
 * style): case-insensitive; returns a score (higher = better) or `null`
 * when `query` is not a subsequence of `text`. Scoring favors
 * consecutive runs, word starts (after space / `-_.:/\`), early first
 * hits, and shorter texts.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase();
  if (q === "") return 0;
  const t = text.toLowerCase();
  let ti = 0;
  let run = 0;
  let first = -1;
  let score = 0;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi];
    const at = t.indexOf(ch, ti);
    if (at === -1) return null;
    if (first === -1) first = at;
    if (at === ti && qi > 0) {
      run += 1;
      score += 8 + run * 2; // consecutive run
    } else {
      run = 0;
      score += 1;
    }
    if (at === 0 || " -_./:\\".includes(t[at - 1])) score += 6; // word start
    ti = at + 1;
  }
  score += Math.max(0, 12 - first); // earlier first hit
  score -= text.length * 0.02; // density: prefer shorter candidates
  return score;
}

/**
 * Filter + rank `items` by fuzzy score against `by(item)`; empty query
 * passes items through in input order. Ties keep input order.
 */
export function fuzzyFilter<T>(
  query: string,
  items: readonly T[],
  by: (item: T) => string,
): T[] {
  const q = query.trim();
  if (q === "") return [...items];
  const scored: { item: T; score: number }[] = [];
  for (const item of items) {
    const score = fuzzyScore(q, by(item));
    if (score !== null) scored.push({ item, score });
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.map((entry) => entry.item);
}
