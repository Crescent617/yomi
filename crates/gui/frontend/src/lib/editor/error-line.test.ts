import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { errorLineField, setErrorLine } from "./error-line";

function stateWith(doc: string) {
  return EditorState.create({ doc, extensions: [errorLineField] });
}

function decoFrom(state: EditorState): number | null {
  let from: number | null = null;
  state.field(errorLineField).between(0, state.doc.length, (start) => {
    from = start;
  });
  return from;
}

describe("errorLineField", () => {
  it("flags the target line", () => {
    let state = stateWith("a = 1\nb = 2\nc = 3\n");
    expect(state.field(errorLineField).size).toBe(0);

    state = state.update({ effects: setErrorLine.of(2) }).state;
    expect(state.field(errorLineField).size).toBe(1);
    expect(decoFrom(state)).toBe(state.doc.line(2).from);
  });

  it("clears the flag on null", () => {
    let state = stateWith("a = 1\nb = 2\n");
    state = state.update({ effects: setErrorLine.of(1) }).state;
    expect(state.field(errorLineField).size).toBe(1);

    state = state.update({ effects: setErrorLine.of(null) }).state;
    expect(state.field(errorLineField).size).toBe(0);
  });

  it("ignores out-of-range line numbers", () => {
    let state = stateWith("a = 1\n");
    state = state.update({ effects: setErrorLine.of(99) }).state;
    expect(state.field(errorLineField).size).toBe(0);
  });

  it("keeps the flag on the flagged line as the document changes", () => {
    let state = stateWith("a = 1\nb = 2\nc = 3\n");
    state = state.update({ effects: setErrorLine.of(2) }).state;

    // Insert a new first line: the flag should move to the old line 2.
    state = state.update({ changes: { from: 0, insert: "new = 0\n" } }).state;
    expect(decoFrom(state)).toBe(state.doc.line(3).from);
  });
});
