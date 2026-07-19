import { describe, expect, test } from "vitest";
import {
  CODEX_PET_ANIMATIONS,
  CODEX_PET_ATLAS_WIDTH,
  CODEX_PET_CELL_HEIGHT,
  CODEX_PET_CELL_WIDTH,
  CODEX_PET_COLUMNS,
  CODEX_PET_LOOK_DEADZONE,
  CODEX_PET_V1_ATLAS_HEIGHT,
  CODEX_PET_V1_ROWS,
  PET_FRAME_DURATION_MULTIPLIER,
  CODEX_PET_V2_ATLAS_HEIGHT,
  CODEX_PET_V2_ROWS,
  getCodexPetFrameDuration,
  getCodexPetFrameGeometry,
  horizontalMovementAnimation,
  moodToCodexPetAnimation,
  resolveCodexPetLookDirection,
  type CodexPetAnimationName,
} from "./codex-pet";

const react_source_frame_durations = {
  idle: [280, 110, 110, 140, 140, 320],
  "running-right": [120, 120, 120, 120, 120, 120, 120, 220],
  "running-left": [120, 120, 120, 120, 120, 120, 120, 220],
  waving: [140, 140, 140, 280],
  jumping: [140, 140, 140, 140, 280],
  failed: [140, 140, 140, 140, 140, 140, 140, 240],
  waiting: [150, 150, 150, 150, 150, 260],
  running: [120, 120, 120, 120, 120, 220],
  review: [150, 150, 150, 150, 150, 280],
} as const satisfies Record<CodexPetAnimationName, readonly number[]>;

const animation_names = Object.keys(
  react_source_frame_durations,
) as CodexPetAnimationName[];

describe("Codex Pets V1 atlas", () => {
  test("uses the exact V1 atlas geometry and represents every row", () => {
    expect({
      width: CODEX_PET_ATLAS_WIDTH,
      height: CODEX_PET_V1_ATLAS_HEIGHT,
      columns: CODEX_PET_COLUMNS,
      rows: CODEX_PET_V1_ROWS,
      cell_width: CODEX_PET_CELL_WIDTH,
      cell_height: CODEX_PET_CELL_HEIGHT,
    }).toEqual({
      width: 1536,
      height: 1872,
      columns: 8,
      rows: 9,
      cell_width: 192,
      cell_height: 208,
    });
    expect(
      animation_names.map((name) => CODEX_PET_ANIMATIONS[name].row),
    ).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    expect(getCodexPetFrameGeometry("review", 5)).toEqual({
      width: 192,
      height: 208,
      background_width: 1536,
      background_height: 1872,
      background_x: -960,
      background_y: -1664,
    });
  });

  test.each(animation_names)(
    "keeps the React source cadence for %s and applies the global multiplier",
    (animation) => {
      const source_durations = react_source_frame_durations[animation];
      const definition = CODEX_PET_ANIMATIONS[animation];

      expect(PET_FRAME_DURATION_MULTIPLIER).toBe(1.5);
      expect(definition.frames).toBe(source_durations.length);
      expect(definition.frame_durations).toEqual(
        source_durations.map(
          (duration) => duration * PET_FRAME_DURATION_MULTIPLIER,
        ),
      );
      expect(
        definition.frame_durations.map((_, frame) =>
          getCodexPetFrameDuration(animation, frame),
        ),
      ).toEqual(definition.frame_durations);
    },
  );

  test("uses the scaled running cadence for movement interactions", () => {
    expect(horizontalMovementAnimation(-1)).toBe("running-left");
    expect(horizontalMovementAnimation(0)).toBeNull();
    expect(horizontalMovementAnimation(1)).toBe("running-right");
    expect(getCodexPetFrameDuration("running-left", 0)).toBe(
      react_source_frame_durations["running-left"][0] *
        PET_FRAME_DURATION_MULTIPLIER,
    );
    expect(getCodexPetFrameDuration("running-right", 0)).toBe(
      react_source_frame_durations["running-right"][0] *
        PET_FRAME_DURATION_MULTIPLIER,
    );
  });

  test("maps application moods to V1 rows", () => {
    expect(moodToCodexPetAnimation("idle")).toBe("idle");
    expect(moodToCodexPetAnimation("working")).toBe("running");
    expect(moodToCodexPetAnimation("happy")).toBe("review");
    expect(moodToCodexPetAnimation("curious")).toBe("waiting");
    expect(moodToCodexPetAnimation("alert")).toBe("waiting");
    expect(moodToCodexPetAnimation("worried")).toBe("failed");
    expect(moodToCodexPetAnimation("sleepy")).toBe("idle");
  });
});

describe("Codex Pets V2 atlas", () => {
  test("uses the 11-row atlas while keeping standard animation rows", () => {
    expect(CODEX_PET_V2_ROWS).toBe(11);
    expect(CODEX_PET_V2_ATLAS_HEIGHT).toBe(2288);
    expect(getCodexPetFrameGeometry("review", 5, 2)).toEqual({
      width: 192,
      height: 208,
      background_width: 1536,
      background_height: 2288,
      background_x: -960,
      background_y: -1664,
    });
  });

  test.each([
    [{ x: 0, y: -1 }, 0, 9, 0],
    [{ x: 1, y: 0 }, 4, 9, 4],
    [{ x: 0, y: 1 }, 8, 10, 0],
    [{ x: -1, y: 0 }, 12, 10, 4],
    [{ x: 1, y: -1 }, 2, 9, 2],
  ] as const)(
    "maps look vector %o to index %i at row %i column %i",
    (look_direction, _look_index, row, column) => {
      const geometry = getCodexPetFrameGeometry("idle", 3, 2, look_direction);
      expect(geometry.background_x).toBe(-column * 192);
      expect(geometry.background_y).toBe(-row * 208);
    },
  );

  test("renders direct look poses while keeping vector gaze as fallback", () => {
    expect(
      getCodexPetFrameGeometry("idle", 3, 2, { x: 1, y: 0 }, 0, 10),
    ).toEqual({
      width: 192,
      height: 208,
      background_width: 1536,
      background_height: 2288,
      background_x: -384,
      background_y: -2080,
    });
    expect(
      getCodexPetFrameGeometry("idle", 3, 2, { x: 1, y: 0 }, 0, 16),
    ).toEqual(getCodexPetFrameGeometry("idle", 3, 2, null, 0, 0));
    expect(
      getCodexPetFrameGeometry("idle", 3, 1, { x: 1, y: 0 }, 0, 10),
    ).toEqual(getCodexPetFrameGeometry("idle", 3, 1));
  });

  test("applies an optional look deadzone", () => {
    expect(CODEX_PET_LOOK_DEADZONE).toBe(24);
    expect(resolveCodexPetLookDirection({ x: 12, y: 0 }, 24)).toBeNull();
    expect(resolveCodexPetLookDirection({ x: 24, y: 0 }, 24)).toBeNull();
    expect(resolveCodexPetLookDirection({ x: 25, y: 0 }, 24)).toBe(4);
    expect(resolveCodexPetLookDirection({ x: 1, y: 0 }, -1)).toBe(4);
  });

  test("ignores look directions for V1 and invalid vectors for V2", () => {
    expect(getCodexPetFrameGeometry("idle", 3, 1, { x: 1, y: 0 })).toEqual(
      getCodexPetFrameGeometry("idle", 3, 1),
    );
    expect(getCodexPetFrameGeometry("idle", 3, 2, { x: 0, y: 0 })).toEqual(
      getCodexPetFrameGeometry("idle", 3, 2),
    );
    expect(getCodexPetFrameGeometry("idle", 3, 2, { x: 12, y: 0 }, 24)).toEqual(
      getCodexPetFrameGeometry("idle", 3, 2),
    );
  });
});
