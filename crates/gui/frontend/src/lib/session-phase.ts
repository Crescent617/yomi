export interface RunningSessionSnapshot {
  id: string;
  phase: string;
}

export interface PhaseSession {
  id: string;
  phase: string;
  phase_revision: number;
}

const ACTIVE_PHASES = new Set(["streaming", "executing_tool", "compacting"]);

export function isActiveSessionPhase(phase: string): boolean {
  return ACTIVE_PHASES.has(phase);
}

// Run start timestamps keyed by session id. Tracked at the phase-event
// level (not in components) so runs that end while their chat view is not
// mounted are still cleared, and switching sessions mid-run keeps the
// clock. Sessions already running when the GUI loads have no entry; the
// inline status falls back to `noteRunStart` for those.
const runStarts = new Map<string, number>();

/** Start time of the session's current run, if known. */
export function getRunStart(sessionId: string): number | undefined {
  return runStarts.get(sessionId);
}

/** Get or lazily create a run start (fallback for pre-observed runs). */
export function noteRunStart(sessionId: string): number {
  const existing = runStarts.get(sessionId);
  if (existing != null) return existing;
  const now = Date.now();
  runStarts.set(sessionId, now);
  return now;
}

/** Record an authoritative lifecycle phase event. */
export function setSessionPhase(session: PhaseSession, phase: string): void {
  const wasActive = isActiveSessionPhase(session.phase);
  const isActive = isActiveSessionPhase(phase);
  session.phase = phase;
  session.phase_revision += 1;
  // Track run boundaries for elapsed-time display: a run starts on the
  // idle→active transition and ends when the session leaves active phases.
  if (isActive && !wasActive) runStarts.set(session.id, Date.now());
  if (!isActive && wasActive) runStarts.delete(session.id);
}

/** Infer a phase from activity without revising an already matching phase. */
export function ensureSessionPhase(session: PhaseSession, phase: string): void {
  if (session.phase !== phase) setSessionPhase(session, phase);
}

export function captureSessionPhaseRevisions<T extends PhaseSession>(
  sessions: T[],
): Map<string, number> {
  return new Map(
    sessions.map((session) => [session.id, session.phase_revision]),
  );
}

/** Apply an async result only if no newer phase event has been observed. */
export function applySessionPhaseIfUnchanged(
  session: PhaseSession,
  phase: string,
  revisionAtRequest: number,
): boolean {
  if (session.phase_revision !== revisionAtRequest) return false;
  setSessionPhase(session, phase);
  return true;
}

/** Apply the conductor's running-session snapshot to loaded GUI sessions. */
export function reconcileRunningSessionPhases<T extends PhaseSession>(
  sessions: T[],
  running: RunningSessionSnapshot[],
  revisionsAtRequest: ReadonlyMap<string, number>,
): void {
  const phaseById = new Map(
    running.map((snapshot) => [snapshot.id, snapshot.phase]),
  );
  for (const session of sessions) {
    const revision = revisionsAtRequest.get(session.id);
    if (revision === undefined) continue;

    const phase = phaseById.get(session.id);
    const nextPhase =
      phase ?? (isActiveSessionPhase(session.phase) ? "idle" : session.phase);
    applySessionPhaseIfUnchanged(session, nextPhase, revision);
  }
}
