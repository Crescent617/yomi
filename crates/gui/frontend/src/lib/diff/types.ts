export interface FileDiff {
  path: string;
  oldContent: string;
  newContent: string;
  hunks: Hunk[];
}

export interface Hunk {
  id: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
  applied: boolean;
}

export interface DiffLine {
  type: "context" | "add" | "remove";
  oldLineNum: number | null;
  newLineNum: number | null;
  content: string;
  intraLineSegments?: IntraSegment[];
}

export interface IntraSegment {
  type: "equal" | "remove" | "add";
  text: string;
}
