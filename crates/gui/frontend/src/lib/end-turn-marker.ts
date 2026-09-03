/**
 * End-of-turn marker (`__YOMI_END_TURN__`).
 *
 * Port of `kernel::prompt::strip_end_turn_marker` — keep the two
 * implementations (and their test cases) in sync. The marker is state
 * machine syntax: stored messages keep the raw text (the kernel reads
 * it there), display strips it. Only the END of the text counts — a
 * marker mid-text is inert and stays visible.
 */

export const END_TURN_MARKER = "__YOMI_END_TURN__";

/**
 * Strip a trailing end-of-turn marker for display. Text not ending
 * with the marker (after trailing-whitespace trim) is returned
 * unchanged.
 */
export function stripEndTurnMarker(text: string): string {
  const trimmed = text.trimEnd();
  if (!trimmed.endsWith(END_TURN_MARKER)) return text;
  return trimmed.slice(0, trimmed.length - END_TURN_MARKER.length).trimEnd();
}
