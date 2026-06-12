export interface FileDiff {
  path: string;
  old_content: string;
  new_content: string;
  hunks: Hunk[];
}

export interface Hunk {
  id: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  lines: DiffLine[];
  applied: boolean;
}

export interface DiffLine {
  type: "context" | "add" | "remove";
  old_line_num: number | null;
  new_line_num: number | null;
  content: string;
  intra_line_segments?: IntraSegment[];
}

export interface IntraSegment {
  type: "equal" | "remove" | "add";
  text: string;
}
