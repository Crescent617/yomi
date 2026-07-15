import { Store } from "@tauri-apps/plugin-store";
import type { PermissionLevel } from "./permission";

const STORAGE_FILE = "yomi-gui-settings";
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
    fontSize: FontSizePreference;
  };
  layout: {
    sidebarCollapsed: boolean;
    sidebarWidth: number;
    sidebar_view: SidebarViewPreference;
  };
  notifications: {
    enabled: boolean;
  };
  chat: {
    homeModel: string | null;
    autoScroll: boolean;
    auto_approve_level: PermissionLevel | null;
    activityGroupExpansion: ActivityGroupExpansionPreference;
  };
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
    fontSize: "base",
  },
  layout: {
    sidebarCollapsed: false,
    sidebarWidth: 256,
    sidebar_view: "projects",
  },
  notifications: {
    enabled: true,
  },
  chat: {
    homeModel: null,
    autoScroll: true,
    auto_approve_level: null,
    activityGroupExpansion: "collapsed",
  },
};

export const guiPreferences = $state<GuiPreferences>(
  cloneGuiPreferences(defaultGuiPreferences),
);

let store: Store | null = null;
let initialized = false;
let initialization: Promise<void> | null = null;

function cloneGuiPreferences(value: GuiPreferences): GuiPreferences {
  return {
    schemaVersion: 1,
    appearance: { ...value.appearance },
    layout: { ...value.layout },
    notifications: { ...value.notifications },
    chat: { ...value.chat },
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
  return {
    schemaVersion: 1,
    appearance: {
      theme: value?.appearance?.theme ?? defaultGuiPreferences.appearance.theme,
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
    },
    notifications: {
      enabled:
        value?.notifications?.enabled ??
        defaultGuiPreferences.notifications.enabled,
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
  };
}

function assignGuiPreferences(value: GuiPreferences): void {
  guiPreferences.schemaVersion = 1;
  Object.assign(guiPreferences.appearance, value.appearance);
  Object.assign(guiPreferences.layout, value.layout);
  Object.assign(guiPreferences.notifications, value.notifications);
  Object.assign(guiPreferences.chat, value.chat);
}

async function getStore(): Promise<Store> {
  if (!store) store = await Store.load(STORAGE_FILE);
  return store;
}

async function loadLegacyPreferences(s: Store): Promise<GuiPreferences> {
  const legacySettings =
    (await s.get<LegacyAppSettings>(LEGACY_SETTINGS_KEY)) ?? {};
  const legacyHomeModel = (await s.get<string>(LEGACY_HOME_MODEL_KEY)) ?? null;

  return normalizeGuiPreferences({
    appearance: {
      theme: legacySettings.theme ?? defaultGuiPreferences.appearance.theme,
      fontSize:
        legacySettings.fontSize ?? defaultGuiPreferences.appearance.fontSize,
    },
    layout: {
      sidebarCollapsed:
        legacySettings.sidebarCollapsed ??
        defaultGuiPreferences.layout.sidebarCollapsed,
      sidebarWidth: defaultGuiPreferences.layout.sidebarWidth,
      sidebar_view: defaultGuiPreferences.layout.sidebar_view,
    },
    notifications: {
      enabled:
        legacySettings.notificationsEnabled ??
        defaultGuiPreferences.notifications.enabled,
    },
    chat: {
      homeModel: legacyHomeModel,
      autoScroll: defaultGuiPreferences.chat.autoScroll,
      auto_approve_level: defaultGuiPreferences.chat.auto_approve_level,
      activityGroupExpansion: defaultGuiPreferences.chat.activityGroupExpansion,
    },
  });
}

async function loadGuiPreferences(): Promise<GuiPreferences> {
  const s = await getStore();
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

export async function saveGuiPreferences(value: GuiPreferences): Promise<void> {
  const normalized = normalizeGuiPreferences(value);
  const s = await getStore();
  await s.set(PREFERENCES_KEY, normalized);
  await s.save();
  assignGuiPreferences(normalized);
  applyGuiPreferences(normalized);
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
