import { describe, expect, test } from "vitest";
import { PET_COMPACT_SIZE, PET_EXPANDED_SIZE, getPetWindowSize } from "./pet";

describe("desktop pet helpers", () => {
  test("uses the compact size without a bubble", () => {
    expect(getPetWindowSize(false)).toBe(PET_COMPACT_SIZE);
    expect(PET_COMPACT_SIZE).toEqual({ width: 152, height: 112 });
  });

  test("expands downward when a bubble is visible", () => {
    expect(getPetWindowSize(true)).toBe(PET_EXPANDED_SIZE);
    expect(PET_EXPANDED_SIZE).toEqual({ width: 200, height: 216 });
    expect(PET_EXPANDED_SIZE.width).toBeGreaterThanOrEqual(
      PET_COMPACT_SIZE.width,
    );
    expect(PET_EXPANDED_SIZE.height).toBeGreaterThan(PET_COMPACT_SIZE.height);
  });
});
