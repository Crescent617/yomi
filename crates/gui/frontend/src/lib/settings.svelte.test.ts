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
    desktop_pet: { ...defaultGuiPreferences.desktop_pet },
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

  test("defaults desktop pet to disabled", () => {
    expect(defaultGuiPreferences.desktop_pet).toEqual({
      enabled: false,
    });
  });

  test("normalizes missing desktop pet preferences", () => {
    const preferences = {
      ...snapshotGuiPreferences(),
      desktop_pet: undefined,
    } as unknown as GuiPreferences;

    replaceGuiPreferences(preferences);

    expect(snapshotGuiPreferences().desktop_pet).toEqual({
      enabled: false,
    });
  });

  test("snapshots desktop pet preferences without sharing references", () => {
    const preferences = snapshotGuiPreferences();
    preferences.desktop_pet.enabled = true;

    expect(snapshotGuiPreferences().desktop_pet.enabled).toBe(false);
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
