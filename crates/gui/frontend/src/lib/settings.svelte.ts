import type { Store } from "@tauri-apps/plugin-store";
import type { PermissionLevel } from "./permission";
import { getSettingsStore } from "./settings-store";
import {
  applyColorThemeById,
  DEFAULT_THEME_ID,
  initCustomThemes,
} from "./themes/themes.svelte";

const PREFERENCES_KEY = "gui_preferences";
const LEGACY_SETTINGS_KEY = "settings";
const LEGACY_HOME_MODEL_KEY = "home_model";

export type ThemePreference = "light" | "dark" | "system";
export type FontSizePreference = "xs" | "sm" | "base" | "lg" | "xl";
export type SidebarViewPreference = "sessions" | "projects";
export type ActivityGroupExpansionPreference =
  | "collapsed"
  | "expanded"
  | "latest"
  | "while_running";

export interface GuiPreferences {
  schemaVersion: 1;
  appearance: {
    theme: ThemePreference;
    /** Color theme id (builtin or custom), resolved by the themes module. */
    theme_id: string;
    fontSize: FontSizePreference;
  };
  layout: {
    sidebarCollapsed: boolean;
    sidebarWidth: number;
    sidebar_view: SidebarViewPreference;
    show_project_sessions_only: boolean;
  };
  notifications: {
    enabled: boolean;
  };
  desktop_pet: {
    enabled: boolean;
    selected_pet_id: string | null;
    scale: number;
  };
  chat: {
    homeModel: string | null;
    autoScroll: boolean;
    auto_approve_level: PermissionLevel | null;
    activityGroupExpansion: ActivityGroupExpansionPreference;
  };
  connection: {
    remote_addr: string | null;
  };
  power: {
    /** Hold an OS power assertion so the machine does not sleep while Yomi runs. */
    keep_awake: boolean;
  };
}

interface LegacyLayoutPreferences extends Partial<GuiPreferences["layout"]> {
  show_all_sessions?: boolean;
}

interface LegacyAppSettings {
  theme?: ThemePreference;
  sidebarCollapsed?: boolean;
  fontSize?: FontSizePreference;
  notificationsEnabled?: boolean;
}

export const defaultGuiPreferences: GuiPreferences = {
  schemaVersion: 1,
  appearance: {
    theme: "system",
    theme_id: DEFAULT_THEME_ID,
    fontSize: "base",
  },
  layout: {
    sidebarCollapsed: false,
    sidebarWidth: 256,
    sidebar_view: "projects",
    show_project_sessions_only: false,
  },
  notifications: {
    enabled: true,
  },
  desktop_pet: {
    enabled: false,
    selected_pet_id: null,
    scale: 1,
  },
  chat: {
    homeModel: null,
    autoScroll: true,
    auto_approve_level: null,
    activityGroupExpansion: "while_running",
  },
  connection: {
    remote_addr: null,
  },
  power: {
    keep_awake: false,
  },
};

export const guiPreferences = $state<GuiPreferences>(
  cloneGuiPreferences(defaultGuiPreferences),
);

let initialized = false;
let initialization: Promise<void> | null = null;
const PREFERENCE_SAVE_DEBOUNCE_MS = 250;
let preferenceSaveTimer: ReturnType<typeof setTimeout> | null = null;
let preferenceSaveQueue: Promise<void> = Promise.resolve();

function cloneGuiPreferences(value: GuiPreferences): GuiPreferences {
  return {
    schemaVersion: 1,
    appearance: { ...value.appearance },
    layout: { ...value.layout },
    notifications: { ...value.notifications },
    desktop_pet: { ...value.desktop_pet },
    chat: { ...value.chat },
    connection: { ...value.connection },
    power: { ...value.power },
  };
}

function clampSidebarWidth(width: number | undefined): number {
  if (!Number.isFinite(width)) return defaultGuiPreferences.layout.sidebarWidth;
  return Math.max(160, Math.min(400, Math.round(width!)));
}

function normalizePermissionLevel(value: unknown): PermissionLevel | null {
  return value === "safe" || value === "caution" || value === "dangerous"
    ? value
    : null;
}

const PET_SCALE_MIN = 0.5;
const PET_SCALE_MAX = 3;

function normalizePetScale(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.min(PET_SCALE_MAX, Math.max(PET_SCALE_MIN, value))
    : defaultGuiPreferences.desktop_pet.scale;
}

function normalizeActivityGroupExpansion(
  value: unknown,
): ActivityGroupExpansionPreference {
  return value === "collapsed" ||
    value === "expanded" ||
    value === "latest" ||
    value === "while_running"
    ? value
    : defaultGuiPreferences.chat.activityGroupExpansion;
}

function normalizeGuiPreferences(
  value?: Partial<GuiPreferences> | null,
): GuiPreferences {
  const layout = value?.layout as LegacyLayoutPreferences | undefined;
  return {
    schemaVersion: 1,
    appearance: {
      theme: value?.appearance?.theme ?? defaultGuiPreferences.appearance.theme,
      theme_id:
        typeof value?.appearance?.theme_id === "string"
          ? value.appearance.theme_id
          : defaultGuiPreferences.appearance.theme_id,
      fontSize:
        value?.appearance?.fontSize ??
        defaultGuiPreferences.appearance.fontSize,
    },
    layout: {
      sidebarCollapsed:
        value?.layout?.sidebarCollapsed ??
        defaultGuiPreferences.layout.sidebarCollapsed,
      sidebarWidth: clampSidebarWidth(value?.layout?.sidebarWidth),
      sidebar_view:
        value?.layout?.sidebar_view === "sessions" ||
        value?.layout?.sidebar_view === "projects"
          ? value.layout.sidebar_view
          : defaultGuiPreferences.layout.sidebar_view,
      show_project_sessions_only:
        layout?.show_project_sessions_only ??
        (typeof layout?.show_all_sessions === "boolean"
          ? !layout.show_all_sessions
          : defaultGuiPreferences.layout.show_project_sessions_only),
    },
    notifications: {
      enabled:
        value?.notifications?.enabled ??
        defaultGuiPreferences.notifications.enabled,
    },
    desktop_pet: {
      enabled:
        value?.desktop_pet?.enabled ??
        defaultGuiPreferences.desktop_pet.enabled,
      selected_pet_id:
        typeof value?.desktop_pet?.selected_pet_id === "string"
          ? value.desktop_pet.selected_pet_id
          : null,
      scale: normalizePetScale(value?.desktop_pet?.scale),
    },
    chat: {
      homeModel: value?.chat?.homeModel ?? defaultGuiPreferences.chat.homeModel,
      autoScroll:
        value?.chat?.autoScroll ?? defaultGuiPreferences.chat.autoScroll,
      auto_approve_level: normalizePermissionLevel(
        value?.chat?.auto_approve_level,
      ),
      activityGroupExpansion: normalizeActivityGroupExpansion(
        value?.chat?.activityGroupExpansion,
      ),
    },
    connection: {
      remote_addr:
        typeof value?.connection?.remote_addr === "string"
          ? value.connection.remote_addr
          : null,
    },
    power: {
      keep_awake:
        value?.power?.keep_awake ?? defaultGuiPreferences.power.keep_awake,
    },
  };
}

function assignGuiPreferences(value: GuiPreferences): void {
  guiPreferences.schemaVersion = 1;
  Object.assign(guiPreferences.appearance, value.appearance);
  Object.assign(guiPreferences.layout, value.layout);
  Object.assign(guiPreferences.notifications, value.notifications);
  Object.assign(guiPreferences.desktop_pet, value.desktop_pet);
  Object.assign(guiPreferences.chat, value.chat);
  Object.assign(guiPreferences.connection, value.connection);
  Object.assign(guiPreferences.power, value.power);
}

async function loadLegacyPreferences(s: Store): Promise<GuiPreferences> {
  const legacySettings =
    (await s.get<LegacyAppSettings>(LEGACY_SETTINGS_KEY)) ?? {};
  const legacyHomeModel = (await s.get<string>(LEGACY_HOME_MODEL_KEY)) ?? null;

  return normalizeGuiPreferences({
    appearance: {
      theme: legacySettings.theme ?? defaultGuiPreferences.appearance.theme,
      theme_id: defaultGuiPreferences.appearance.theme_id,
      fontSize:
        legacySettings.fontSize ?? defaultGuiPreferences.appearance.fontSize,
    },
    layout: {
      sidebarCollapsed:
        legacySettings.sidebarCollapsed ??
        defaultGuiPreferences.layout.sidebarCollapsed,
      sidebarWidth: defaultGuiPreferences.layout.sidebarWidth,
      sidebar_view: defaultGuiPreferences.layout.sidebar_view,
      show_project_sessions_only:
        defaultGuiPreferences.layout.show_project_sessions_only,
    },
    notifications: {
      enabled:
        legacySettings.notificationsEnabled ??
        defaultGuiPreferences.notifications.enabled,
    },
    desktop_pet: {
      enabled: defaultGuiPreferences.desktop_pet.enabled,
      selected_pet_id: defaultGuiPreferences.desktop_pet.selected_pet_id,
      scale: defaultGuiPreferences.desktop_pet.scale,
    },
    chat: {
      homeModel: legacyHomeModel,
      autoScroll: defaultGuiPreferences.chat.autoScroll,
      auto_approve_level: defaultGuiPreferences.chat.auto_approve_level,
      activityGroupExpansion: defaultGuiPreferences.chat.activityGroupExpansion,
    },
    connection: {
      remote_addr: defaultGuiPreferences.connection.remote_addr,
    },
    power: {
      keep_awake: defaultGuiPreferences.power.keep_awake,
    },
  });
}

async function loadGuiPreferences(): Promise<GuiPreferences> {
  const s = await getSettingsStore();
  const saved = await s.get<Partial<GuiPreferences>>(PREFERENCES_KEY);
  if (saved) return normalizeGuiPreferences(saved);

  const migrated = await loadLegacyPreferences(s);
  await s.set(PREFERENCES_KEY, migrated);
  await s.save();
  return migrated;
}

export async function initSettings(): Promise<void> {
  if (initialized) return;
  if (initialization) return initialization;

  initialization = (async () => {
    // Custom themes must be loaded before the active palette can resolve.
    await initCustomThemes();
    try {
      const loaded = await loadGuiPreferences();
      assignGuiPreferences(loaded);
    } catch (error) {
      console.error("[Settings] Failed to load GUI preferences:", error);
      assignGuiPreferences(defaultGuiPreferences);
    }
    applyGuiPreferences(guiPreferences);
    initialized = true;
  })();

  try {
    await initialization;
  } finally {
    initialization = null;
  }
}

export function snapshotGuiPreferences(): GuiPreferences {
  return cloneGuiPreferences($state.snapshot(guiPreferences));
}

export function replaceGuiPreferences(value: GuiPreferences): void {
  const normalized = normalizeGuiPreferences(value);
  assignGuiPreferences(normalized);
  applyGuiPreferences(normalized);
}

async function persistGuiPreferences(
  normalized: GuiPreferences,
): Promise<void> {
  const s = await getSettingsStore();
  await s.set(PREFERENCES_KEY, normalized);
  await s.save();
}

function enqueueGuiPreferencesSave(value: GuiPreferences): Promise<void> {
  const normalized = normalizeGuiPreferences(value);
  const save = preferenceSaveQueue.then(() =>
    persistGuiPreferences(normalized),
  );
  preferenceSaveQueue = save.catch(() => undefined);
  return save;
}

export async function saveGuiPreferences(value: GuiPreferences): Promise<void> {
  if (preferenceSaveTimer) {
    clearTimeout(preferenceSaveTimer);
    preferenceSaveTimer = null;
  }
  const normalized = normalizeGuiPreferences(value);
  // Apply eagerly: state is the source of truth, persistence is just I/O.
  // An awaited save's values must be visible to the next snapshot (e.g.
  // select theme, then toggle light/dark) — without this, a debounced
  // snapshot captured right after resurrects the pre-save value and
  // clobbers this write in the store.
  assignGuiPreferences(normalized);
  applyGuiPreferences(normalized);
  await enqueueGuiPreferencesSave(normalized);
}

export function scheduleGuiPreferencesSave(): void {
  if (preferenceSaveTimer) clearTimeout(preferenceSaveTimer);
  preferenceSaveTimer = setTimeout(() => {
    preferenceSaveTimer = null;
    // State is the source of truth: mutate `guiPreferences` first, then
    // schedule. Snapshot at fire time so mutations made inside the
    // debounce window are persisted too.
    void enqueueGuiPreferencesSave(snapshotGuiPreferences()).catch((error) => {
      console.error("[Settings] Failed to save GUI preferences:", error);
    });
  }, PREFERENCE_SAVE_DEBOUNCE_MS);
}

export function applyGuiPreferences(value: GuiPreferences): void {
  applyTheme(value.appearance.theme);
  applyFontSize(value.appearance.fontSize);
}

export function applyTheme(theme: ThemePreference): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  const dark =
    theme === "dark" ||
    (theme === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.classList.toggle("dark", dark);
  // Re-apply the active color theme so its palette matches the new mode.
  applyColorThemeById(guiPreferences.appearance.theme_id, dark);
  window.dispatchEvent(new CustomEvent("theme-changed", { detail: { theme } }));
}

export function applyFontSize(fontSize: FontSizePreference): void {
  if (typeof document === "undefined") return;
  const sizes: Record<FontSizePreference, string> = {
    xs: "13px",
    sm: "14px",
    base: "16px",
    lg: "18px",
    xl: "20px",
  };
  document.documentElement.style.fontSize = sizes[fontSize];
}

let mediaQuery: MediaQueryList | null = null;
let mediaListener: ((event: MediaQueryListEvent) => void) | null = null;

export function startThemeListener(): void {
  if (typeof window === "undefined" || mediaQuery) return;
  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  mediaListener = () => {
    if (guiPreferences.appearance.theme === "system") applyTheme("system");
  };
  mediaQuery.addEventListener("change", mediaListener);
}

export function stopThemeListener(): void {
  if (!mediaQuery || !mediaListener) return;
  mediaQuery.removeEventListener("change", mediaListener);
  mediaQuery = null;
  mediaListener = null;
}

// Compatibility helpers for components that only need the home model.
export async function getHomeModel(): Promise<string | null> {
  await initSettings();
  return guiPreferences.chat.homeModel;
}

export async function setHomeModel(model: string): Promise<void> {
  const next = snapshotGuiPreferences();
  next.chat.homeModel = model;
  await saveGuiPreferences(next);
}
