import { beforeEach, describe, expect, test } from "vitest";
import {
  defaultGuiPreferences,
  replaceGuiPreferences,
  snapshotGuiPreferences,
  type GuiPreferences,
} from "./settings.svelte";

function preferencesWithActivityGroupExpansion(
  activityGroupExpansion: unknown,
): GuiPreferences {
  return {
    ...defaultGuiPreferences,
    appearance: { ...defaultGuiPreferences.appearance },
    layout: { ...defaultGuiPreferences.layout },
    notifications: { ...defaultGuiPreferences.notifications },
    chat: {
      ...defaultGuiPreferences.chat,
      activityGroupExpansion,
    },
  } as GuiPreferences;
}

describe("GUI preference normalization", () => {
  beforeEach(() => {
    replaceGuiPreferences(defaultGuiPreferences);
  });

  test("defaults activity group expansion to while_running", () => {
    expect(defaultGuiPreferences.chat.activityGroupExpansion).toBe(
      "while_running",
    );
  });

  test("normalizes a missing activity group expansion", () => {
    const preferences = preferencesWithActivityGroupExpansion(undefined);

    replaceGuiPreferences(preferences);

    expect(snapshotGuiPreferences().chat.activityGroupExpansion).toBe(
      "while_running",
    );
  });

  test("normalizes an invalid activity group expansion", () => {
    const preferences = preferencesWithActivityGroupExpansion("sometimes");

    replaceGuiPreferences(preferences);

    expect(snapshotGuiPreferences().chat.activityGroupExpansion).toBe(
      "while_running",
    );
  });
});
