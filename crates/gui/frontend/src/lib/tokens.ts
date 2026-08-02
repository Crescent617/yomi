// Token estimation mirroring the kernel's `utils::tokens` heuristics so
// both sides agree on the numbers:
// - plain text (thinking): 1 token ≈ 4 UTF-8 bytes
// - JSON (tool call arguments): 1 token ≈ 2 UTF-8 bytes (denser punctuation)
//
// These are estimates; the provider only reports exact usage at stream end.

const encoder = new TextEncoder();

export const utf8Length = (text: string): number => encoder.encode(text).length;

/** Output estimate from accumulated stream bytes (in-flight response). */
export function estimateStreamTokens(
  textBytes: number,
  jsonBytes: number,
): number {
  return Math.ceil(textBytes / 4) + Math.ceil(jsonBytes / 2);
}
