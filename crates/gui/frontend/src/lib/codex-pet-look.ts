import type { PetMood } from "./api";
import {
  CODEX_PET_LOOK_DIRECTIONS,
  PET_FRAME_DURATION_MULTIPLIER,
  normalizeCodexPetLookIndex,
} from "./codex-pet";
import type { PetSpriteVersion } from "./pet";

export { normalizeCodexPetLookIndex };

export interface CodexPetLookAvailability {
  sprite_version_number: PetSpriteVersion;
  mood: PetMood;
  has_interaction: boolean;
  reduced_motion: boolean;
  visible: boolean;
}

export type CodexPetLookSource = "gaze" | "ambient" | null;

export type CodexPetLookMode =
  | "ineligible"
  | "ambient_wait"
  | "ambient_cooldown"
  | "gaze_hold"
  | "ambient_cycle";

export interface CodexPetLookOutput {
  source: CodexPetLookSource;
  look_index: number | null;
  mode: CodexPetLookMode;
}

export interface CodexPetLookPolicy {
  gaze_hold_ms: number;
  ambient_initial_delay_ms: readonly [number, number];
  ambient_cooldown_ms: readonly [number, number];
  ambient_pose_source_ms: number;
}

export interface ResolvedCodexPetLookPolicy extends CodexPetLookPolicy {
  ambient_pose_ms: number;
}

export interface CodexPetLookControllerOptions {
  random?: () => number;
  policy?: Partial<CodexPetLookPolicy>;
}

export const YOMI_PET_LOOK_POLICY: CodexPetLookPolicy = {
  gaze_hold_ms: 1_500,
  ambient_initial_delay_ms: [8_000, 18_000],
  ambient_cooldown_ms: [12_000, 30_000],
  // The source cadence is scaled by Yomi's global pet animation multiplier.
  ambient_pose_source_ms: 120,
};

const AMBIENT_LOOK_CYCLE_COUNT = 2;
const AMBIENT_LOOK_POSE_COUNT =
  CODEX_PET_LOOK_DIRECTIONS * AMBIENT_LOOK_CYCLE_COUNT;

type WaitMode = "ambient_wait" | "ambient_cooldown";

interface AmbientCycle {
  pose_step: number;
  deadline: number;
}

/** V2 look poses are available while the pet uses its idle animation. */
export function isCodexPetLookEligible({
  sprite_version_number,
  mood,
  has_interaction,
  reduced_motion,
  visible,
}: CodexPetLookAvailability): boolean {
  return (
    sprite_version_number === 2 &&
    (mood === "idle" || mood === "sleepy") &&
    !has_interaction &&
    !reduced_motion &&
    visible
  );
}

export class CodexPetLookController {
  readonly policy: ResolvedCodexPetLookPolicy;

  private readonly random: () => number;
  private eligible = false;
  private last_now: number | null = null;
  private gaze_index: number | null = null;
  private gaze_deadline: number | null = null;
  private ambient_due: number | null = null;
  private wait_mode: WaitMode = "ambient_wait";
  private ambient: AmbientCycle | null = null;

  constructor({
    random = Math.random,
    policy = {},
  }: CodexPetLookControllerOptions = {}) {
    this.random = random;
    this.policy = resolvePolicy(policy);
  }

  /** Changes raw eligibility and returns the output at `now`. */
  set_eligibility(
    availability: CodexPetLookAvailability,
    now: number,
  ): CodexPetLookOutput {
    return this.set_eligible(isCodexPetLookEligible(availability), now);
  }

  /** Changes precomputed eligibility and returns the output at `now`. */
  set_eligible(eligible: boolean, now: number): CodexPetLookOutput {
    this.recordNow(now);
    if (eligible === this.eligible) {
      return eligible ? this.advance(now) : this.currentOutput();
    }

    this.eligible = eligible;
    this.clearActivity();
    if (eligible) this.scheduleInitial(now);
    return this.currentOutput();
  }

  /** Restarts ambient timing without changing the current eligibility. */
  restart(now: number): CodexPetLookOutput {
    this.recordNow(now);
    this.clearActivity();
    if (this.eligible) this.scheduleInitial(now);
    return this.currentOutput();
  }

  /**
   * Reports cursor movement. A null/invalid index is a deadzone event: it still
   * cancels ambient behavior and postpones it, but does not start gaze.
   */
  cursor_moved(look_index: number | null, now: number): CodexPetLookOutput {
    this.recordNow(now);
    if (!this.eligible) return this.currentOutput();

    this.ambient = null;
    this.scheduleInitial(now);
    this.gaze_index =
      look_index === null ? null : normalizeCodexPetLookIndex(look_index);
    this.gaze_deadline =
      this.gaze_index === null ? null : now + this.policy.gaze_hold_ms;
    return this.currentOutput();
  }

  /** Ends cursor gaze without disabling or postponing ambient behavior. */
  cursor_unavailable(now: number): CodexPetLookOutput {
    this.recordNow(now);
    this.gaze_index = null;
    this.gaze_deadline = null;
    if (this.ambient_due !== null && this.ambient_due < now) {
      this.ambient_due = now;
    }
    return this.advance(now);
  }

  /** Advances all deadline-driven state without using timers or browser APIs. */
  tick(now: number): CodexPetLookOutput {
    this.recordNow(now);
    return this.advance(now);
  }

  get_output(): CodexPetLookOutput {
    return this.currentOutput();
  }

  private advance(now: number): CodexPetLookOutput {
    if (!this.eligible) return this.currentOutput();

    // Every transition advances an absolute deadline, so sparse ticks produce
    // the same state and random choices as ticks at every individual deadline.
    while (true) {
      if (this.gaze_index !== null && this.gaze_deadline !== null) {
        if (now < this.gaze_deadline) return this.currentOutput();
        const gaze_deadline = this.gaze_deadline;
        this.gaze_index = null;
        this.gaze_deadline = null;
        if (this.ambient_due !== null && this.ambient_due < gaze_deadline) {
          this.ambient_due = gaze_deadline;
        }
      }

      if (this.ambient !== null) {
        if (now < this.ambient.deadline) return this.currentOutput();
        const deadline = this.ambient.deadline;
        if (this.ambient.pose_step < AMBIENT_LOOK_POSE_COUNT - 1) {
          this.ambient.pose_step += 1;
          this.ambient.deadline = deadline + this.policy.ambient_pose_ms;
        } else {
          this.ambient = null;
          this.scheduleCooldown(deadline);
        }
        continue;
      }

      if (this.ambient_due === null || now < this.ambient_due) {
        return this.currentOutput();
      }

      this.startAmbient(this.ambient_due);
    }
  }

  private startAmbient(started_at: number): void {
    this.ambient_due = null;
    this.ambient = {
      pose_step: 0,
      deadline: started_at + this.policy.ambient_pose_ms,
    };
  }

  private scheduleInitial(now: number): void {
    this.ambient_due =
      now + this.randomRange(this.policy.ambient_initial_delay_ms);
    this.wait_mode = "ambient_wait";
  }

  private scheduleCooldown(now: number): void {
    this.ambient_due = now + this.randomRange(this.policy.ambient_cooldown_ms);
    this.wait_mode = "ambient_cooldown";
  }

  private randomRange([minimum, maximum]: readonly [number, number]): number {
    return minimum + this.randomUnit() * (maximum - minimum);
  }

  private randomUnit(): number {
    const value = this.random();
    if (!Number.isFinite(value)) return 0;
    return Math.max(0, Math.min(1, value));
  }

  private clearActivity(): void {
    this.gaze_index = null;
    this.gaze_deadline = null;
    this.ambient_due = null;
    this.ambient = null;
    this.wait_mode = "ambient_wait";
  }

  private currentOutput(): CodexPetLookOutput {
    if (!this.eligible) {
      return { source: null, look_index: null, mode: "ineligible" };
    }
    if (this.gaze_index !== null) {
      return {
        source: "gaze",
        look_index: this.gaze_index,
        mode: "gaze_hold",
      };
    }
    if (this.ambient !== null) {
      return {
        source: "ambient",
        look_index: this.ambient.pose_step % CODEX_PET_LOOK_DIRECTIONS,
        mode: "ambient_cycle",
      };
    }
    return { source: null, look_index: null, mode: this.wait_mode };
  }

  private recordNow(now: number): void {
    if (!Number.isFinite(now)) {
      throw new RangeError("CodexPetLookController time must be finite");
    }
    if (this.last_now !== null && now < this.last_now) {
      throw new RangeError(
        "CodexPetLookController time must not move backwards",
      );
    }
    this.last_now = now;
  }
}

function resolvePolicy(
  overrides: Partial<CodexPetLookPolicy>,
): ResolvedCodexPetLookPolicy {
  const policy = { ...YOMI_PET_LOOK_POLICY, ...overrides };

  validateNonnegative("gaze_hold_ms", policy.gaze_hold_ms);
  validateRange("ambient_initial_delay_ms", policy.ambient_initial_delay_ms);
  validateRange("ambient_cooldown_ms", policy.ambient_cooldown_ms);
  validatePositive("ambient_pose_source_ms", policy.ambient_pose_source_ms);

  return {
    ...policy,
    ambient_pose_ms:
      policy.ambient_pose_source_ms * PET_FRAME_DURATION_MULTIPLIER,
  };
}

function validateRange(name: string, range: readonly [number, number]): void {
  if (
    range.length !== 2 ||
    !Number.isFinite(range[0]) ||
    !Number.isFinite(range[1]) ||
    range[0] < 0 ||
    range[1] < range[0]
  ) {
    throw new RangeError(
      `${name} must be a nonnegative [minimum, maximum] range`,
    );
  }
}

function validateNonnegative(name: string, value: number): void {
  if (!Number.isFinite(value) || value < 0) {
    throw new RangeError(`${name} must be nonnegative`);
  }
}

function validatePositive(name: string, value: number): void {
  if (!Number.isFinite(value) || value <= 0) {
    throw new RangeError(`${name} must be positive`);
  }
}
