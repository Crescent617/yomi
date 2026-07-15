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

/** Record an authoritative lifecycle phase event. */
export function setSessionPhase(session: PhaseSession, phase: string): void {
  session.phase = phase;
  session.phase_revision += 1;
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
