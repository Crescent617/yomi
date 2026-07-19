import type { PetMood } from "./api";
import type { PetSpriteVersion } from "./pet";

export const CODEX_PET_ATLAS_WIDTH = 1536;
export const CODEX_PET_V1_ATLAS_HEIGHT = 1872;
export const CODEX_PET_V2_ATLAS_HEIGHT = 2288;
export const CODEX_PET_COLUMNS = 8;
export const CODEX_PET_V1_ROWS = 9;
export const CODEX_PET_V2_ROWS = 11;
export const CODEX_PET_CELL_WIDTH = 192;
export const CODEX_PET_CELL_HEIGHT = 208;
export const CODEX_PET_LOOK_DIRECTIONS = 16;
export const CODEX_PET_LOOK_STEP_DEGREES = 22.5;
export const CODEX_PET_LOOK_DEADZONE = 24;

export interface CodexPetLookVector {
  x: number;
  y: number;
}

export type CodexPetAnimationName =
  | "idle"
  | "running-right"
  | "running-left"
  | "waving"
  | "jumping"
  | "failed"
  | "waiting"
  | "running"
  | "review";

export interface CodexPetAnimation {
  row: number;
  frames: number;
  frame_durations: readonly number[];
}

export const PET_FRAME_DURATION_MULTIPLIER = 1.5;

const CODEX_PET_REACT_FRAME_DURATIONS = {
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

function scaleFrameDurations(durations: readonly number[]): number[] {
  return durations.map((duration) => duration * PET_FRAME_DURATION_MULTIPLIER);
}

// Codex Pets V1 atlas contract and source frame cadence from codex-pets-react at
// a851f87ee0cbb1d923a26d40fcad717f32dc58ec. Yomi applies one global duration
// multiplier so the original per-frame rhythm is preserved at a calmer pace.
// Shared Codex Pets V1/V2 standard animation rows. V2 appends two rows of
// static look-direction poses; their selection and timing belong to the host.
export const CODEX_PET_ANIMATIONS = {
  idle: {
    row: 0,
    frames: 6,
    frame_durations: scaleFrameDurations(CODEX_PET_REACT_FRAME_DURATIONS.idle),
  },
  "running-right": {
    row: 1,
    frames: 8,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS["running-right"],
    ),
  },
  "running-left": {
    row: 2,
    frames: 8,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS["running-left"],
    ),
  },
  waving: {
    row: 3,
    frames: 4,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.waving,
    ),
  },
  jumping: {
    row: 4,
    frames: 5,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.jumping,
    ),
  },
  failed: {
    row: 5,
    frames: 8,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.failed,
    ),
  },
  waiting: {
    row: 6,
    frames: 6,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.waiting,
    ),
  },
  running: {
    row: 7,
    frames: 6,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.running,
    ),
  },
  review: {
    row: 8,
    frames: 6,
    frame_durations: scaleFrameDurations(
      CODEX_PET_REACT_FRAME_DURATIONS.review,
    ),
  },
} as const satisfies Record<CodexPetAnimationName, CodexPetAnimation>;

export interface CodexPetFrameGeometry {
  width: number;
  height: number;
  background_width: number;
  background_height: number;
  background_x: number;
  background_y: number;
}

export function getCodexPetFrameGeometry(
  animation: CodexPetAnimationName,
  frame: number,
  sprite_version_number: PetSpriteVersion = 1,
  look_direction: CodexPetLookVector | null = null,
  look_deadzone = 0,
  look_index: number | null = null,
): CodexPetFrameGeometry {
  const definition = CODEX_PET_ANIMATIONS[animation];
  const safe_frame = Math.max(0, Math.min(definition.frames - 1, frame));
  const resolved_look_index =
    sprite_version_number === 2
      ? (normalizeCodexPetLookIndex(look_index) ??
        resolveCodexPetLookDirection(look_direction, look_deadzone))
      : null;
  const row =
    resolved_look_index === null
      ? definition.row
      : CODEX_PET_V1_ROWS + Math.floor(resolved_look_index / CODEX_PET_COLUMNS);
  const column =
    resolved_look_index === null
      ? safe_frame
      : resolved_look_index % CODEX_PET_COLUMNS;
  return {
    width: CODEX_PET_CELL_WIDTH,
    height: CODEX_PET_CELL_HEIGHT,
    background_width: CODEX_PET_ATLAS_WIDTH,
    background_height:
      sprite_version_number === 2
        ? CODEX_PET_V2_ATLAS_HEIGHT
        : CODEX_PET_V1_ATLAS_HEIGHT,
    background_x: -column * CODEX_PET_CELL_WIDTH,
    background_y: -row * CODEX_PET_CELL_HEIGHT,
  };
}

/** Normalizes an integer (or null) to one of the 16 clockwise look indices. */
export function normalizeCodexPetLookIndex(
  look_index: number | null,
): number | null {
  if (look_index === null || !Number.isInteger(look_index)) return null;
  return (
    ((look_index % CODEX_PET_LOOK_DIRECTIONS) + CODEX_PET_LOOK_DIRECTIONS) %
    CODEX_PET_LOOK_DIRECTIONS
  );
}

/** Quantizes a screen-space vector to a clockwise V2 look index starting at up. */
export function resolveCodexPetLookDirection(
  direction: CodexPetLookVector | null | undefined,
  deadzone = 0,
): number | null {
  if (
    !direction ||
    !Number.isFinite(direction.x) ||
    !Number.isFinite(direction.y) ||
    Math.hypot(direction.x, direction.y) <= Math.max(0, deadzone)
  ) {
    return null;
  }

  const degrees =
    ((Math.atan2(direction.x, -direction.y) * 180) / Math.PI + 360) % 360;
  return (
    Math.round(degrees / CODEX_PET_LOOK_STEP_DEGREES) %
    CODEX_PET_LOOK_DIRECTIONS
  );
}

export function getCodexPetFrameDuration(
  animation: CodexPetAnimationName,
  frame: number,
): number {
  const definition = CODEX_PET_ANIMATIONS[animation];
  const safe_frame = Math.max(0, Math.min(definition.frames - 1, frame));
  return definition.frame_durations[safe_frame];
}

export function horizontalMovementAnimation(
  delta_x: number,
): CodexPetAnimationName | null {
  if (delta_x < 0) return "running-left";
  if (delta_x > 0) return "running-right";
  return null;
}

export function moodToCodexPetAnimation(mood: PetMood): CodexPetAnimationName {
  switch (mood) {
    case "working":
      return "running";
    case "happy":
      return "review";
    case "curious":
    case "alert":
      return "waiting";
    case "worried":
      return "failed";
    case "idle":
    case "sleepy":
      return "idle";
  }
}
