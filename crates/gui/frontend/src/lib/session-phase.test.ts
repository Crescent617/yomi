import { describe, expect, test } from "vitest";
import {
  applySessionPhaseIfUnchanged,
  captureSessionPhaseRevisions,
  isActiveSessionPhase,
  reconcileRunningSessionPhases,
  setSessionPhase,
  type PhaseSession,
} from "./session-phase";

function phaseSession(id: string, phase = "idle"): PhaseSession {
  return { id, phase, phase_revision: 0 };
}

describe("isActiveSessionPhase", () => {
  test.each(["streaming", "executing_tool", "compacting"])(
    "treats %s as active",
    (phase) => expect(isActiveSessionPhase(phase)).toBe(true),
  );

  test.each(["idle", "closed", "completed", "error"])(
    "treats %s as inactive",
    (phase) => expect(isActiveSessionPhase(phase)).toBe(false),
  );
});

describe("reconcileRunningSessionPhases", () => {
  test("restores active phases from the running snapshot", () => {
    const sessions = [phaseSession("streaming"), phaseSession("tool")];
    const revisions = captureSessionPhaseRevisions(sessions);

    reconcileRunningSessionPhases(
      sessions,
      [
        { id: "streaming", phase: "streaming" },
        { id: "tool", phase: "executing_tool" },
      ],
      revisions,
    );

    expect(sessions.map(({ id, phase }) => ({ id, phase }))).toEqual([
      { id: "streaming", phase: "streaming" },
      { id: "tool", phase: "executing_tool" },
    ]);
  });

  test("clears stale active phases absent from the running snapshot", () => {
    const session = phaseSession("finished", "streaming");
    const revisions = captureSessionPhaseRevisions([session]);

    reconcileRunningSessionPhases([session], [], revisions);

    expect(session.phase).toBe("idle");
  });

  test("does not clear an active phase received after the snapshot request", () => {
    const session = phaseSession("started-late");
    const revisions = captureSessionPhaseRevisions([session]);
    setSessionPhase(session, "streaming");

    reconcileRunningSessionPhases([session], [], revisions);

    expect(session.phase).toBe("streaming");
  });

  test("does not restore an active phase after a newer idle event", () => {
    const session = phaseSession("finished-during-request", "streaming");
    const revisions = captureSessionPhaseRevisions([session]);
    setSessionPhase(session, "idle");

    reconcileRunningSessionPhases(
      [session],
      [{ id: session.id, phase: "streaming" }],
      revisions,
    );

    expect(session.phase).toBe("idle");
  });

  test("rejects an old snapshot after an idle-to-streaming ABA cycle", () => {
    const session = phaseSession("new-run", "streaming");
    const revisions = captureSessionPhaseRevisions([session]);
    setSessionPhase(session, "idle");
    setSessionPhase(session, "streaming");

    reconcileRunningSessionPhases([session], [], revisions);

    expect(session.phase).toBe("streaming");
    expect(session.phase_revision).toBe(2);
  });
});

describe("applySessionPhaseIfUnchanged", () => {
  test("does not let a stale idle query overwrite a newer active event", () => {
    const session = phaseSession("started");
    const revision = session.phase_revision;
    setSessionPhase(session, "streaming");

    expect(applySessionPhaseIfUnchanged(session, "idle", revision)).toBe(false);
    expect(session.phase).toBe("streaming");
  });

  test("does not let a stale active query overwrite a newer idle event", () => {
    const session = phaseSession("finished", "streaming");
    const revision = session.phase_revision;
    setSessionPhase(session, "idle");

    expect(applySessionPhaseIfUnchanged(session, "streaming", revision)).toBe(
      false,
    );
    expect(session.phase).toBe("idle");
  });

  test("records repeated authoritative phase events as newer revisions", () => {
    const session = phaseSession("repeated", "streaming");

    setSessionPhase(session, "streaming");

    expect(session.phase).toBe("streaming");
    expect(session.phase_revision).toBe(1);
  });

  test("applies a query when no newer phase update exists", () => {
    const session = phaseSession("unchanged");

    expect(applySessionPhaseIfUnchanged(session, "compacting", 0)).toBe(true);
    expect(session).toEqual({
      id: "unchanged",
      phase: "compacting",
      phase_revision: 1,
    });
  });
});
