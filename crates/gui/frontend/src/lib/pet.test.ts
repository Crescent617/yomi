import { describe, expect, test } from "vitest";
import { PET_SIZE } from "./pet";

describe("desktop pet sizing", () => {
  test("uses one fixed Codex Pets cell for both sprite versions", () => {
    expect(PET_SIZE).toEqual({ width: 192, height: 208 });
  });
});
