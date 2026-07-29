import { beforeEach, describe, expect, test, vi } from "vitest";

const storeMocks = vi.hoisted(() => ({
  set: vi.fn(),
  save: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-store", () => ({
  Store: {
    load: vi.fn(async () => storeMocks),
  },
}));

import {
  defaultGuiPreferences,
  replaceGuiPreferences,
  saveGuiPreferences,
  scheduleGuiPreferencesSave,
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
  beforeEach(async () => {
    vi.useRealTimers();
    storeMocks.set.mockReset();
    storeMocks.save.mockReset();
    replaceGuiPreferences(defaultGuiPreferences);
    await saveGuiPreferences(snapshotGuiPreferences());
    storeMocks.set.mockClear();
    storeMocks.save.mockClear();
  });

  test("defaults activity group expansion to while_running", () => {
    expect(defaultGuiPreferences.chat.activityGroupExpansion).toBe(
      "while_running",
    );
  });

  test("defaults desktop pet to disabled", () => {
    expect(defaultGuiPreferences.desktop_pet).toEqual({
      enabled: false,
      selected_pet_id: null,
      scale: 1,
    });
  });

  test("defaults keep-awake to disabled", () => {
    expect(defaultGuiPreferences.power).toEqual({ keep_awake: false });
  });

  test("normalizes missing power preferences", () => {
    const preferences = {
      ...snapshotGuiPreferences(),
      power: undefined,
    } as unknown as GuiPreferences;

    replaceGuiPreferences(preferences);

    expect(snapshotGuiPreferences().power).toEqual({ keep_awake: false });
  });

  test("snapshots power preferences without sharing references", () => {
    const preferences = snapshotGuiPreferences();
    preferences.power.keep_awake = true;

    expect(snapshotGuiPreferences().power.keep_awake).toBe(false);
  });

  test("normalizes missing desktop pet preferences", () => {
    const preferences = {
      ...snapshotGuiPreferences(),
      desktop_pet: undefined,
    } as unknown as GuiPreferences;

    replaceGuiPreferences(preferences);

    expect(snapshotGuiPreferences().desktop_pet).toEqual({
      enabled: false,
      selected_pet_id: null,
      scale: 1,
    });
  });

  test("normalizes missing and invalid selected pet IDs", () => {
    const missing = {
      ...snapshotGuiPreferences(),
      desktop_pet: { enabled: true },
    } as unknown as GuiPreferences;
    replaceGuiPreferences(missing);
    expect(snapshotGuiPreferences().desktop_pet).toEqual({
      enabled: true,
      selected_pet_id: null,
      scale: 1,
    });

    const invalid = {
      ...snapshotGuiPreferences(),
      desktop_pet: { enabled: true, selected_pet_id: 42 },
    } as unknown as GuiPreferences;
    replaceGuiPreferences(invalid);
    expect(snapshotGuiPreferences().desktop_pet.selected_pet_id).toBeNull();
  });

  test("normalizes invalid and out-of-range pet scales", () => {
    const with_scale = (scale: unknown) =>
      ({
        ...snapshotGuiPreferences(),
        desktop_pet: { ...snapshotGuiPreferences().desktop_pet, scale },
      }) as GuiPreferences;

    replaceGuiPreferences(with_scale("big"));
    expect(snapshotGuiPreferences().desktop_pet.scale).toBe(1);
    replaceGuiPreferences(with_scale(Number.NaN));
    expect(snapshotGuiPreferences().desktop_pet.scale).toBe(1);
    replaceGuiPreferences(with_scale(0.1));
    expect(snapshotGuiPreferences().desktop_pet.scale).toBe(0.5);
    replaceGuiPreferences(with_scale(10));
    expect(snapshotGuiPreferences().desktop_pet.scale).toBe(3);
    replaceGuiPreferences(with_scale(1.5));
    expect(snapshotGuiPreferences().desktop_pet.scale).toBe(1.5);
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

  test("debounces preference saves and persists only the latest state", async () => {
    vi.useFakeTimers();
    const first = snapshotGuiPreferences();
    first.layout.sidebarWidth = 220;
    replaceGuiPreferences(first);
    scheduleGuiPreferencesSave();
    const latest = snapshotGuiPreferences();
    latest.layout.sidebarWidth = 320;
    replaceGuiPreferences(latest);
    scheduleGuiPreferencesSave();
    expect(storeMocks.set).not.toHaveBeenCalled();

    await vi.runAllTimersAsync();

    expect(storeMocks.set).toHaveBeenCalledTimes(1);
    expect(storeMocks.set).toHaveBeenCalledWith(
      "gui_preferences",
      expect.objectContaining({
        layout: expect.objectContaining({ sidebarWidth: 320 }),
      }),
    );
    expect(storeMocks.save).toHaveBeenCalledTimes(1);
  });

  test("a scheduled save persists state mutated after scheduling", async () => {
    // State is the source of truth: the debounced save snapshots at fire
    // time, so mutations inside the debounce window are persisted too.
    vi.useFakeTimers();
    scheduleGuiPreferencesSave();
    const mutated = snapshotGuiPreferences();
    mutated.appearance.theme = "dark";
    replaceGuiPreferences(mutated);

    await vi.runAllTimersAsync();

    expect(storeMocks.set).toHaveBeenCalledTimes(1);
    expect(storeMocks.set).toHaveBeenCalledWith(
      "gui_preferences",
      expect.objectContaining({
        appearance: expect.objectContaining({ theme: "dark" }),
      }),
    );
  });

  test("an immediate save cancels a pending debounced save", async () => {
    vi.useFakeTimers();
    const pending = snapshotGuiPreferences();
    pending.layout.sidebarWidth = 220;
    replaceGuiPreferences(pending);
    scheduleGuiPreferencesSave();
    const immediate = snapshotGuiPreferences();
    immediate.layout.sidebarWidth = 360;

    await saveGuiPreferences(immediate);
    await vi.runAllTimersAsync();

    expect(storeMocks.set).toHaveBeenCalledTimes(1);
    expect(storeMocks.set).toHaveBeenCalledWith(
      "gui_preferences",
      expect.objectContaining({
        layout: expect.objectContaining({ sidebarWidth: 360 }),
      }),
    );
  });

  test("an awaited save is visible to the next snapshot", async () => {
    // Regression for the select-then-toggle race: the debounced snapshot
    // captured right after an awaited save must carry the saved values,
    // not resurrect the previous ones and clobber them in the store.
    vi.useFakeTimers();
    const selected = snapshotGuiPreferences();
    selected.appearance.theme_id = "custom-x";
    await saveGuiPreferences(selected);

    const next = snapshotGuiPreferences();
    expect(next.appearance.theme_id).toBe("custom-x");

    next.appearance.theme = "dark";
    replaceGuiPreferences(next);
    scheduleGuiPreferencesSave();
    await vi.runAllTimersAsync();

    expect(storeMocks.set).toHaveBeenLastCalledWith(
      "gui_preferences",
      expect.objectContaining({
        appearance: expect.objectContaining({
          theme: "dark",
          theme_id: "custom-x",
        }),
      }),
    );
  });
});
