export interface DeleteToLineStartResult {
  /** Start index of the range to remove. */
  start: number;
  /** End index of the range to remove. */
  end: number;
  /** Caret position after the deletion. */
  cursor: number;
}

/**
 * Compute the range removed by Cmd+Backspace (delete to beginning of line).
 *
 * With an active selection, the selection itself is removed (matching common
 * editor behavior). Otherwise everything between the start of the current
 * line and the caret is removed. Returns a no-op range (`start === end`)
 * when the caret is already at the start of a line.
 *
 * All indices are UTF-16 code units, matching textarea selection semantics;
 * the computed boundaries never split a surrogate pair.
 */
export function deleteToLineStart(
  value: string,
  selectionStart: number,
  selectionEnd: number,
): DeleteToLineStartResult {
  if (selectionStart !== selectionEnd) {
    return { start: selectionStart, end: selectionEnd, cursor: selectionStart };
  }
  const start = value.lastIndexOf("\n", selectionStart - 1) + 1;
  return { start, end: selectionStart, cursor: start };
}
