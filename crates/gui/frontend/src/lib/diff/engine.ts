import type { FileDiff, Hunk, DiffLine, IntraSegment } from "./types";
import diffMatchPatch from "diff-match-patch";

const dmp = new diffMatchPatch();

export function computeFileDiff(
  path: string,
  old_content: string,
  new_content: string,
): FileDiff {
  // Simple line-level diff using LCS-like approach
  const hunks = computeHunks(old_content.split("\n"), new_content.split("\n"));

  return {
    path,
    old_content: old_content,
    new_content: new_content,
    hunks,
  };
}

function computeHunks(old_lines: string[], new_lines: string[]): Hunk[] {
  const hunks: Hunk[] = [];
  let oldIdx = 0;
  let newIdx = 0;
  let hunkLines: DiffLine[] = [];
  let hunkOldStart = 0;
  let hunkNewStart = 0;
  let inHunk = false;

  // Simple diff: compare line by line
  while (oldIdx < old_lines.length || newIdx < new_lines.length) {
    const oldLine = oldIdx < old_lines.length ? old_lines[oldIdx] : undefined;
    const newLine = newIdx < new_lines.length ? new_lines[newIdx] : undefined;

    if (oldLine === newLine) {
      // Same line
      if (inHunk) {
        // Add trailing context (up to 3 lines)
        if (hunkLines.filter((l) => l.type !== "context").length > 0) {
          hunks.push(createHunk(hunkOldStart, hunkNewStart, hunkLines));
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
            old_line_num: i + 1,
            new_line_num: i + 1,
            content: old_lines[i],
          });
        }
      }

      if (oldLine !== undefined && newLine !== undefined) {
        // Check if this is a modification (same position)
        hunkLines.push({
          type: "remove",
          old_line_num: oldIdx + 1,
          new_line_num: null,
          content: oldLine,
        });
        hunkLines.push({
          type: "add",
          old_line_num: null,
          new_line_num: newIdx + 1,
          content: newLine,
        });
        oldIdx++;
        newIdx++;
      } else if (oldLine !== undefined) {
        // Deleted line
        hunkLines.push({
          type: "remove",
          old_line_num: oldIdx + 1,
          new_line_num: null,
          content: oldLine,
        });
        oldIdx++;
      } else {
        // Added line
        hunkLines.push({
          type: "add",
          old_line_num: null,
          new_line_num: newIdx + 1,
          content: newLine!,
        });
        newIdx++;
      }
    }
  }

  if (inHunk && hunkLines.filter((l) => l.type !== "context").length > 0) {
    hunks.push(createHunk(hunkOldStart, hunkNewStart, hunkLines));
  }

  return hunks;
}

function createHunk(
  old_start: number,
  new_start: number,
  lines: DiffLine[],
): Hunk {
  const oldCount = lines.filter((l) => l.type !== "add").length;
  const newCount = lines.filter((l) => l.type !== "remove").length;

  // Compute intra-line diff for modified line pairs
  const processedLines = lines.map((line, idx) => {
    if (line.type === "add") {
      // Find corresponding remove line
      const prevLine = lines[idx - 1];
      if (prevLine && prevLine.type === "remove") {
        line.intra_line_segments = computeIntraLineDiff(
          prevLine.content,
          line.content,
        );
      }
    }
    return line;
  });

  return {
    id: `hunk-${old_start}-${new_start}`,
    old_start: old_start,
    old_lines: oldCount,
    new_start: new_start,
    new_lines: newCount,
    lines: processedLines,
    applied: true,
  };
}

function computeIntraLineDiff(
  oldText: string,
  newText: string,
): IntraSegment[] {
  const diffs = dmp.diff_main(oldText, newText);
  dmp.diff_cleanupSemantic(diffs);

  return diffs.map(([type, text]: [number, string]) => {
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
  const old_lines = diff.old_content.split("\n");
  const lines: string[] = [];
  let lineIdx = 0;

  for (const hunk of diff.hunks) {
    // Skip to hunk start
    while (lineIdx < hunk.new_start - 1) {
      lines.push(old_lines[lineIdx] ?? "");
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
