import { describe, expect, test } from "vitest";
import { activityGroupExpanded } from "./activity-expansion";

describe("activity group expansion", () => {
  test("supports all automatic policies", () => {
    expect(activityGroupExpanded("collapsed", true, true, null)).toBe(false);
    expect(activityGroupExpanded("expanded", false, false, null)).toBe(true);
    expect(activityGroupExpanded("latest", true, false, null)).toBe(true);
    expect(activityGroupExpanded("latest", false, true, null)).toBe(false);
    expect(activityGroupExpanded("while_running", false, true, null)).toBe(
      true,
    );
    expect(activityGroupExpanded("while_running", true, false, null)).toBe(
      false,
    );
  });

  test("manual choice overrides the automatic policy", () => {
    expect(activityGroupExpanded("collapsed", false, false, "open")).toBe(true);
    expect(activityGroupExpanded("expanded", true, true, "closed")).toBe(false);
  });
});
