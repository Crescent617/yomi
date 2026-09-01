// Shared stack of open overlay layers (modals, previews, lightboxes) so
// global key handling (Escape) only ever targets the top-most one: an
// overlay pushes a unique token while open and handles Escape only when
// its token is last. See Modal.svelte for the originating pattern.
//
// Caveat: the stack orders *input*, not *paint* — z-index alignment is
// each layer's own responsibility (MermaidPreview uses z-[100], the
// rest z-50). A layer pushed above a higher-z layer would be
// Escape-top yet invisible; keep new layers' z consistent with their
// typical push order. The command palette is not a layer: it suppresses
// itself while any layer is open (hasOpenModal) instead of stacking.
const stack: symbol[] = [];

/** Push `id` on open; the returned cleanup removes it (on close). */
export function pushModal(id: symbol): () => void {
  stack.push(id);
  return () => {
    const idx = stack.indexOf(id);
    if (idx !== -1) stack.splice(idx, 1);
  };
}

/** Whether `id` is the top-most open layer. */
export function isTopModal(id: symbol): boolean {
  return stack[stack.length - 1] === id;
}

/** Whether any overlay layer is currently open. */
export function hasOpenModal(): boolean {
  return stack.length > 0;
}
