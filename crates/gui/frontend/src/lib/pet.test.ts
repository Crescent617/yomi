import { describe, expect, test } from "vitest";
import { PET_BUBBLE_SIZE } from "./pet";

describe("desktop pet helpers", () => {
  test("uses the compact bubble window size", () => {
    expect(PET_BUBBLE_SIZE).toEqual({ width: 280, height: 160 });
  });
});
