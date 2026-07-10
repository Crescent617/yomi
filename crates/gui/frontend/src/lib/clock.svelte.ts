// ── Shared reactive clock ────────────────────────────────────────────────
// A minute-level tick so relative timestamps ("5m ago") stay fresh without
// each component managing its own interval.

export const clock = $state({ now: Date.now() });

let started = false;

/** Start the global minute tick (idempotent). */
export function startClock() {
  if (started) return;
  started = true;
  setInterval(() => {
    clock.now = Date.now();
  }, 30_000);
}
