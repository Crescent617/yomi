const openingFence = /^( {0,3})(`{3,}|~{3,})[^\S\r\n]*(.*)$/;

/** Whether the current stream ends exactly after a valid closing backtick fence. */
export function endsWithClosedBacktickFence(markdown: string): boolean {
  const lastLineStart = markdown.lastIndexOf("\n") + 1;
  const lastLine = markdown.slice(lastLineStart).replace(/\r$/, "");
  if (!/^ {0,3}`{3,}[^\S\r\n]*$/.test(lastLine)) return false;

  const lines = markdown.split(/\r?\n/);
  let openLength: number | undefined;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (openLength === undefined) {
      const match = openingFence.exec(line);
      if (!match || match[2][0] !== "`") continue;
      const info = match[3].trimStart();
      if (info.includes("`")) continue;
      openLength = match[2].length;
      continue;
    }

    const candidate = line.trimStart();
    const indent = line.length - candidate.length;
    const closing = candidate.trimEnd() === "`".repeat(openLength);
    if (indent > 3 || !closing) continue;
    if (index === lines.length - 1) return true;
    openLength = undefined;
  }

  return false;
}

/** Count Mermaid fenced code blocks whose closing fence has arrived. */
export function countClosedMermaidFences(markdown: string): number {
  const lines = markdown.split(/\r?\n/);
  let open: { marker: "`" | "~"; length: number; mermaid: boolean } | undefined;
  let count = 0;

  for (const line of lines) {
    if (!open) {
      const match = openingFence.exec(line);
      if (!match) continue;
      const fence = match[2];
      const info = match[3].trimStart();
      // Backtick fence info strings cannot contain a backtick.
      if (fence[0] === "`" && info.includes("`")) continue;
      open = {
        marker: fence[0] as "`" | "~",
        length: fence.length,
        mermaid: info.split(/\s+/, 1)[0]?.toLowerCase() === "mermaid",
      };
      continue;
    }

    const closing = new RegExp(
      `^ {0,3}\\${open.marker}{${open.length},}[^\\S\\r\\n]*$`,
    );
    if (!closing.test(line)) continue;
    if (open.mermaid) count += 1;
    open = undefined;
  }

  return count;
}
