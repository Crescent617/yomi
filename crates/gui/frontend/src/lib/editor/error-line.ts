import { StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";

/** Flag (or clear, with `null`) the line a save diagnostic points at. */
export const setErrorLine = StateEffect.define<number | null>();

const errorLineMark = Decoration.line({ class: "cm-error-line" });

export const errorLineField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(decorations, tr) {
    decorations = decorations.map(tr.changes);
    for (const effect of tr.effects) {
      if (!effect.is(setErrorLine)) continue;
      if (
        effect.value !== null &&
        effect.value >= 1 &&
        effect.value <= tr.state.doc.lines
      ) {
        decorations = Decoration.set([
          errorLineMark.range(tr.state.doc.line(effect.value).from),
        ]);
      } else {
        decorations = Decoration.none;
      }
    }
    return decorations;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** Move the cursor to a 1-based line/column, flag the line, and scroll it into view. */
export function jumpToLine(view: EditorView, line: number, column = 1): void {
  const safeLine = Math.max(1, Math.min(line, view.state.doc.lines));
  const { from, to } = view.state.doc.line(safeLine);
  const pos = Math.min(from + Math.max(0, column - 1), to);
  view.dispatch({
    selection: { anchor: pos },
    effects: [
      setErrorLine.of(safeLine),
      EditorView.scrollIntoView(pos, { y: "center" }),
    ],
  });
  view.focus();
}
