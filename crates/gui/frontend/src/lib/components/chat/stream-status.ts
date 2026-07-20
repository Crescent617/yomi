// Token estimation for the inline stream status, mirroring the kernel's
// `utils::tokens` heuristics so both sides agree on the numbers:
// - plain text (thinking): 1 token ≈ 4 UTF-8 bytes
// - JSON (tool call arguments): 1 token ≈ 2 UTF-8 bytes (denser punctuation)
//
// These are estimates; the provider only reports exact usage at stream end.

const encoder = new TextEncoder();

const utf8Length = (text: string): number => encoder.encode(text).length;

export function estimateTextTokens(text: string): number {
  return Math.ceil(utf8Length(text) / 4);
}

export function estimateJsonTokens(text: string): number {
  return Math.ceil(utf8Length(text) / 2);
}

// Display format mirroring the kernel's `format_estimated_tokens`:
// `~` prefix marks the value as an estimate.
export function formatStreamTokens(count: number): string {
  const word = count === 1 ? "token" : "tokens";
  if (count >= 1000) return `~${(count / 1000).toFixed(1)}k ${word}`;
  return `~${count} ${word}`;
}
