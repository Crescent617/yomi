/**
 * Table column width fitting for the share card.
 *
 * Columns start at their natural (single-line content) width. When the sum
 * exceeds the available width, space is distributed proportionally to the
 * natural widths — mirroring how the chat body's HTML tables behave — with
 * a minimum column width as the floor. Columns clamped to the floor hand
 * their deficit back to the rest, so the distribution iterates to a fixpoint.
 */

/** Minimum width a column shrinks to. */
export const TABLE_MIN_COL_W = 48;

/**
 * Fit `natural` column widths into `inner` px of usable width.
 * Returns a copy of the input unchanged when everything fits; otherwise
 * returns proportionally shrunk widths, each >= TABLE_MIN_COL_W whenever
 * the budget allows.
 */
export function fitTableColumnWidths(
  natural: number[],
  inner: number,
): number[] {
  const total = natural.reduce((a, b) => a + b, 0);
  if (total <= inner) return natural.slice();

  const widths = natural.slice();
  // Columns at/below the floor keep their natural width from the start.
  const frozen = natural.map((w) => w <= TABLE_MIN_COL_W);
  // Each pass freezes at least one more column, so this terminates.
  for (let iter = 0; iter < natural.length; iter++) {
    let frozenW = 0;
    let flexW = 0;
    let flexCount = 0;
    for (let k = 0; k < natural.length; k++) {
      if (frozen[k]) frozenW += widths[k];
      else {
        flexW += natural[k];
        flexCount++;
      }
    }
    if (flexCount === 0) break; // everything floored; may still overflow
    const budget = inner - frozenW;
    let clampedAny = false;
    for (let k = 0; k < natural.length; k++) {
      if (frozen[k]) continue;
      // Non-frozen columns are > TABLE_MIN_COL_W, so flexW > 0 here.
      const share = (natural[k] / flexW) * budget;
      if (share <= TABLE_MIN_COL_W) {
        widths[k] = TABLE_MIN_COL_W;
        frozen[k] = true;
        clampedAny = true;
      } else {
        widths[k] = share;
      }
    }
    if (!clampedAny) break;
  }
  return widths;
}
