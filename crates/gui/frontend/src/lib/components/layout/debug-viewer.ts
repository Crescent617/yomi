export function appendNewContent(
  current: string,
  currentEnd: number,
  chunk: { content: string; start_offset: number; end_offset: number },
): string {
  if (chunk.start_offset > currentEnd || chunk.end_offset < currentEnd) {
    return chunk.content;
  }
  const overlapBytes = currentEnd - chunk.start_offset;
  if (overlapBytes <= 0) return current + chunk.content;
  const encoded = new TextEncoder().encode(chunk.content);
  const suffix = new TextDecoder().decode(encoded.slice(overlapBytes));
  return current + suffix;
}

export function prependEarlierContent(
  earlier: string,
  current: string,
): string {
  return earlier + current;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
