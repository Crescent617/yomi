const openingFence = /^( {0,3})(`{3,}|~{3,})[^\S\r\n]*(.*)$/;

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
