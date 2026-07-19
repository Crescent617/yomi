import { describe, expect, test } from "vitest";
import { PET_FRAME_DURATION_MULTIPLIER } from "./codex-pet";
import {
  CodexPetLookController,
  isCodexPetLookEligible,
  normalizeCodexPetLookIndex,
  type CodexPetLookAvailability,
  type CodexPetLookPolicy,
} from "./codex-pet-look";

const eligible: CodexPetLookAvailability = {
  sprite_version_number: 2,
  mood: "idle",
  has_interaction: false,
  reduced_motion: false,
  visible: true,
};

function controllerWith(
  policy: Partial<CodexPetLookPolicy>,
  random: () => number = () => 0,
): CodexPetLookController {
  return new CodexPetLookController({ random, policy });
}

describe("Codex pet look eligibility", () => {
  test.each([
    ["eligible V2 idle pet", eligible, true],
    ["sleepy pet using idle animation", { ...eligible, mood: "sleepy" }, true],
    ["V1 pet", { ...eligible, sprite_version_number: 1 }, false],
    ["non-idle mood", { ...eligible, mood: "working" }, false],
    ["active interaction", { ...eligible, has_interaction: true }, false],
    ["reduced motion", { ...eligible, reduced_motion: true }, false],
    ["hidden pet", { ...eligible, visible: false }, false],
  ] as const)("handles %s", (_name, availability, expected) => {
    expect(isCodexPetLookEligible(availability)).toBe(expected);
  });
});

describe("Codex pet look policy", () => {
  test("uses Yomi timing and scales the ambient pose cadence", () => {
    const controller = new CodexPetLookController();

    expect(controller.policy).toMatchObject({
      gaze_hold_ms: 1_500,
      ambient_initial_delay_ms: [8_000, 18_000],
      ambient_cooldown_ms: [12_000, 30_000],
      ambient_pose_source_ms: 120,
      ambient_pose_ms: 180,
    });
    expect(controller.policy.ambient_pose_ms).toBe(
      120 * PET_FRAME_DURATION_MULTIPLIER,
    );
  });

  test("uses injected random values at delay bounds", () => {
    const policy = {
      ambient_initial_delay_ms: [100, 200] as const,
      ambient_cooldown_ms: [300, 400] as const,
      ambient_pose_source_ms: 10,
    };
    const minimum = controllerWith(policy);
    minimum.set_eligible(true, 0);
    expect(minimum.tick(99).source).toBeNull();
    expect(minimum.tick(100).look_index).toBe(0);

    const maximum = controllerWith(policy, () => 1);
    maximum.set_eligible(true, 0);
    expect(maximum.tick(199).source).toBeNull();
    expect(maximum.tick(200).look_index).toBe(0);
  });

  test("supports overrides and rejects invalid policy", () => {
    const controller = controllerWith({
      gaze_hold_ms: 25,
      ambient_initial_delay_ms: [30, 30],
      ambient_pose_source_ms: 10,
    });
    expect(controller.policy.gaze_hold_ms).toBe(25);
    expect(controller.policy.ambient_pose_ms).toBe(15);

    expect(
      () =>
        new CodexPetLookController({
          policy: { ambient_initial_delay_ms: [20, 10] },
        }),
    ).toThrow(RangeError);
    expect(
      () =>
        new CodexPetLookController({
          policy: { ambient_pose_source_ms: 0 },
        }),
    ).toThrow(RangeError);
  });
});

describe("Codex pet cursor gaze", () => {
  test("normalizes gaze, gives it priority, and resets the hold", () => {
    const controller = controllerWith({
      gaze_hold_ms: 1_500,
      ambient_initial_delay_ms: [0, 0],
      ambient_pose_source_ms: 100,
    });
    controller.set_eligible(true, 0);

    expect(controller.cursor_moved(18, 0)).toEqual({
      source: "gaze",
      look_index: 2,
      mode: "gaze_hold",
    });
    expect(controller.tick(999).look_index).toBe(2);
    expect(controller.cursor_moved(-1, 1_000).look_index).toBe(15);
    expect(controller.tick(2_499).source).toBe("gaze");
    expect(controller.tick(2_500)).toEqual({
      source: "ambient",
      look_index: 0,
      mode: "ambient_cycle",
    });
  });

  test("treats null movement as a deadzone without gaze", () => {
    const controller = controllerWith({
      ambient_initial_delay_ms: [500, 500],
    });
    controller.set_eligible(true, 0);
    controller.cursor_moved(3, 100);

    expect(controller.cursor_moved(null, 200)).toEqual({
      source: null,
      look_index: null,
      mode: "ambient_wait",
    });
    expect(controller.tick(699).source).toBeNull();
    expect(controller.tick(700).look_index).toBe(0);
  });
});

describe("Codex pet ambient look-around", () => {
  test("plays every clockwise pose twice, then enters cooldown", () => {
    const controller = controllerWith({
      ambient_initial_delay_ms: [100, 100],
      ambient_cooldown_ms: [1_000, 1_000],
      ambient_pose_source_ms: 120,
    });
    controller.set_eligible(true, 0);

    expect(controller.tick(99).source).toBeNull();
    expect(controller.tick(100)).toEqual({
      source: "ambient",
      look_index: 0,
      mode: "ambient_cycle",
    });
    for (let pose_step = 0; pose_step < 32; pose_step += 1) {
      const started_at = 100 + pose_step * 180;
      const look_index = pose_step % 16;
      expect(controller.tick(started_at).look_index).toBe(look_index);
      expect(controller.tick(started_at + 179).look_index).toBe(look_index);
    }
    expect(controller.tick(5_860)).toEqual({
      source: null,
      look_index: null,
      mode: "ambient_cooldown",
    });
    expect(controller.tick(6_859).source).toBeNull();
    expect(controller.tick(6_860).look_index).toBe(0);
  });

  test("catches up through multiple expired cycles from absolute deadlines", () => {
    const policy: Partial<CodexPetLookPolicy> = {
      ambient_initial_delay_ms: [100, 100],
      ambient_cooldown_ms: [200, 200],
      ambient_pose_source_ms: 10,
    };
    const frequent = controllerWith(policy);
    const sparse = controllerWith(policy);
    frequent.set_eligible(true, 0);
    sparse.set_eligible(true, 0);

    for (let now = 100; now <= 1_000; now += 5) frequent.tick(now);

    expect(sparse.tick(1_000)).toEqual(frequent.get_output());
  });

  test("cursor movement interrupts the cycle and restarts the idle delay", () => {
    const controller = controllerWith({
      gaze_hold_ms: 100,
      ambient_initial_delay_ms: [500, 500],
      ambient_pose_source_ms: 100,
    });
    controller.set_eligible(true, 0);
    expect(controller.tick(500).look_index).toBe(0);
    expect(controller.tick(650).look_index).toBe(1);

    expect(controller.cursor_moved(9, 700)).toEqual({
      source: "gaze",
      look_index: 9,
      mode: "gaze_hold",
    });
    expect(controller.tick(800).mode).toBe("ambient_wait");
    expect(controller.tick(1_199).source).toBeNull();
    expect(controller.tick(1_200).look_index).toBe(0);
  });

  test("ineligibility clears a cycle and reschedules on re-entry", () => {
    const controller = controllerWith({
      ambient_initial_delay_ms: [100, 100],
    });
    controller.set_eligible(true, 0);
    expect(controller.tick(100).look_index).toBe(0);

    expect(controller.set_eligible(false, 200)).toEqual({
      source: null,
      look_index: null,
      mode: "ineligible",
    });
    expect(controller.set_eligible(true, 5_000).mode).toBe("ambient_wait");
    expect(controller.tick(5_099).source).toBeNull();
    expect(controller.tick(5_100).look_index).toBe(0);
  });

  test("restart clears a cycle while preserving eligibility", () => {
    const controller = controllerWith({
      ambient_initial_delay_ms: [100, 100],
    });
    controller.set_eligible(true, 0);
    expect(controller.tick(100).look_index).toBe(0);

    expect(controller.restart(200).mode).toBe("ambient_wait");
    expect(controller.tick(299).source).toBeNull();
    expect(controller.tick(300).look_index).toBe(0);
  });

  test("cursor unavailability ends gaze but preserves future ambient", () => {
    const controller = controllerWith({
      gaze_hold_ms: 1_000,
      ambient_initial_delay_ms: [500, 500],
    });
    controller.set_eligible(true, 0);
    controller.cursor_moved(6, 0);

    expect(controller.cursor_unavailable(200).mode).toBe("ambient_wait");
    expect(controller.tick(499).source).toBeNull();
    expect(controller.tick(500).look_index).toBe(0);
  });

  test("rejects non-finite and backwards time", () => {
    const controller = new CodexPetLookController();
    controller.set_eligible(true, 100);

    expect(() => controller.tick(99)).toThrow(/must not move backwards/);
    expect(() => controller.tick(Number.NaN)).toThrow(/must be finite/);
  });
});

describe("Codex pet look index", () => {
  test("normalizes integer indices and rejects non-integers", () => {
    expect(normalizeCodexPetLookIndex(16)).toBe(0);
    expect(normalizeCodexPetLookIndex(-17)).toBe(15);
    expect(normalizeCodexPetLookIndex(2.5)).toBeNull();
  });
});
