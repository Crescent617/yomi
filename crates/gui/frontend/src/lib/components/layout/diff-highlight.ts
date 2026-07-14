import type { ThemedToken } from "shiki";
import { highlightCodeTokens } from "../chat/code-highlight";
import { detectLang } from "../../utils";

export type HighlightedDiffLineType = "context" | "add" | "del" | "hunk";

export interface HighlightedDiffLine {
  type: HighlightedDiffLineType;
  text: string;
  oldTokens?: ThemedToken[];
  newTokens?: ThemedToken[];
}

export interface HighlightedDiffHunk {
  lines: HighlightedDiffLine[];
}

export function diffSources(lines: HighlightedDiffLine[]): {
  oldSource: string;
  newSource: string;
} {
  return {
    oldSource: lines
      .filter((line) => line.type !== "add")
      .map((line) => line.text)
      .join("\n"),
    newSource: lines
      .filter((line) => line.type !== "del")
      .map((line) => line.text)
      .join("\n"),
  };
}

export function resolveDiffLanguagePath(
  oldPath: string | undefined,
  newPath: string | undefined,
  fallbackPath: string,
): string {
  if (newPath && newPath !== "/dev/null") return newPath;
  if (oldPath && oldPath !== "/dev/null") return oldPath;
  return fallbackPath;
}

/**
 * Tokenize old and new hunk sources independently so additions inherit new-file
 * grammar state and deletions inherit old-file grammar state.
 */
export async function highlightDiffHunks(
  hunks: HighlightedDiffHunk[],
  filePath: string,
): Promise<void> {
  const language = detectLang(filePath);
  if (language === "plaintext") return;

  await Promise.all(
    hunks.map(async (hunk) => {
      const { oldSource, newSource } = diffSources(hunk.lines);
      const [oldLines, newLines] = await Promise.all([
        highlightCodeTokens(oldSource, language),
        highlightCodeTokens(newSource, language),
      ]);
      if (!oldLines || !newLines) return;

      let oldIndex = 0;
      let newIndex = 0;
      for (const line of hunk.lines) {
        if (line.type !== "add") {
          line.oldTokens = oldLines[oldIndex] ?? [];
          oldIndex += 1;
        }
        if (line.type !== "del") {
          line.newTokens = newLines[newIndex] ?? [];
          newIndex += 1;
        }
      }
    }),
  );
}

export function tokenStyle(token: ThemedToken): string | undefined {
  if (!token.htmlStyle) return token.color ? `color:${token.color}` : undefined;
  if (typeof token.htmlStyle === "string") return token.htmlStyle;
  return Object.entries(token.htmlStyle)
    .map(([property, value]) => `${property}:${value}`)
    .join(";");
}
