// Shared stack of open overlay layers (modals, previews, lightboxes) so
// global key handling (Escape) only ever targets the top-most one: an
// overlay pushes a unique token while open and handles Escape only when
// its token is last. See Modal.svelte for the originating pattern.
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
