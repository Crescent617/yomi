export function thinkingPreview(content: string): string {
  return content.replace(/\s+/g, " ").trim();
}
