import type { FileDiff, Hunk, DiffLine, IntraSegment } from "./types";
import diffMatchPatch from "diff-match-patch";

const dmp = new diffMatchPatch();

export function computeFileDiff(
  path: string,
  oldContent: string,
  newContent: string
): FileDiff {
  // Simple line-level diff using LCS-like approach
  const hunks = computeHunks(
    oldContent.split("\n"),
    newContent.split("\n")
  );

  return {
    path,
    oldContent,
    newContent,
    hunks,
  };
}

function computeHunks(oldLines: string[], newLines: string[]): Hunk[] {
  const hunks: Hunk[] = [];
  let oldIdx = 0;
  let newIdx = 0;
  let hunkLines: DiffLine[] = [];
  let hunkOldStart = 0;
  let hunkNewStart = 0;
  let inHunk = false;

  // Simple diff: compare line by line
  while (oldIdx < oldLines.length || newIdx < newLines.length) {
    const oldLine = oldIdx < oldLines.length ? oldLines[oldIdx] : undefined;
    const newLine = newIdx < newLines.length ? newLines[newIdx] : undefined;

    if (oldLine === newLine) {
      // Same line
      if (inHunk) {
        // Add trailing context (up to 3 lines)
        if (hunkLines.filter((l) => l.type !== "context").length > 0) {
          hunks.push(
            createHunk(
              hunkOldStart,
              hunkNewStart,
              hunkLines
            )
          );
        }
        inHunk = false;
        hunkLines = [];
      }
      oldIdx++;
      newIdx++;
    } else {
      // Different
      if (!inHunk) {
        inHunk = true;
        hunkOldStart = oldIdx + 1;
        hunkNewStart = newIdx + 1;
        // Add leading context (up to 3 lines)
        const ctxStart = Math.max(0, oldIdx - 3);
        for (let i = ctxStart; i < oldIdx; i++) {
          hunkLines.push({
            type: "context",
            oldLineNum: i + 1,
            newLineNum: i + 1,
            content: oldLines[i],
          });
        }
      }

      if (oldLine !== undefined && newLine !== undefined) {
        // Check if this is a modification (same position)
        hunkLines.push({
          type: "remove",
          oldLineNum: oldIdx + 1,
          newLineNum: null,
          content: oldLine,
        });
        hunkLines.push({
          type: "add",
          oldLineNum: null,
          newLineNum: newIdx + 1,
          content: newLine,
        });
        oldIdx++;
        newIdx++;
      } else if (oldLine !== undefined) {
        // Deleted line
        hunkLines.push({
          type: "remove",
          oldLineNum: oldIdx + 1,
          newLineNum: null,
          content: oldLine,
        });
        oldIdx++;
      } else {
        // Added line
        hunkLines.push({
          type: "add",
          oldLineNum: null,
          newLineNum: newIdx + 1,
          content: newLine!,
        });
        newIdx++;
      }
    }
  }

  if (inHunk && hunkLines.filter((l) => l.type !== "context").length > 0) {
    hunks.push(
      createHunk(hunkOldStart, hunkNewStart, hunkLines)
    );
  }

  return hunks;
}

function createHunk(
  oldStart: number,
  newStart: number,
  lines: DiffLine[]
): Hunk {
  const oldCount = lines.filter((l) => l.type !== "add").length;
  const newCount = lines.filter((l) => l.type !== "remove").length;

  // Compute intra-line diff for modified line pairs
  const processedLines = lines.map((line, idx) => {
    if (line.type === "add") {
      // Find corresponding remove line
      const prevLine = lines[idx - 1];
      if (prevLine && prevLine.type === "remove") {
        line.intraLineSegments = computeIntraLineDiff(
          prevLine.content,
          line.content
        );
      }
    }
    return line;
  });

  return {
    id: `hunk-${oldStart}-${newStart}`,
    oldStart,
    oldLines: oldCount,
    newStart,
    newLines: newCount,
    lines: processedLines,
    applied: true,
  };
}

function computeIntraLineDiff(oldText: string, newText: string): IntraSegment[] {
  const diffs = dmp.diff_main(oldText, newText);
  dmp.diff_cleanupSemantic(diffs);

  return diffs.map(([type, text]) => {
    const typeStr = type === -1 ? "remove" : type === 1 ? "add" : "equal";
    return { type: typeStr as IntraSegment["type"], text };
  });
}

export function filterAppliedHunks(diff: FileDiff): FileDiff {
  return {
    ...diff,
    hunks: diff.hunks.filter((h) => h.applied),
  };
}

export function reconstructContent(diff: FileDiff): string {
  const lines: string[] = [];
  let lineIdx = 0;

  for (const hunk of diff.hunks) {
    // Skip to hunk start
    while (lineIdx < hunk.newStart - 1) {
      lines.push(diff.oldContent.split("\n")[lineIdx] ?? "");
      lineIdx++;
    }

    for (const line of hunk.lines) {
      if (line.type === "context" || line.type === "add") {
        lines.push(line.content);
        lineIdx++;
      }
    }
  }

  return lines.join("\n");
}
