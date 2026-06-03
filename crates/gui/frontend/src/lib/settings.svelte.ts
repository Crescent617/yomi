import { Store } from "@tauri-apps/plugin-store";

const STORAGE_KEY = "yomi-gui-settings";

export interface AppSettings {
  theme: "light" | "dark" | "system";
  sidebarCollapsed: boolean;
  fontSize: "sm" | "base" | "lg";
  notificationsEnabled: boolean;
}

const defaults: AppSettings = {
  theme: "system",
  sidebarCollapsed: false,
  fontSize: "base",
  notificationsEnabled: true,
};

let store: Store | null = null;

async function getStore(): Promise<Store> {
  if (store) return store;
  store = await Store.load(STORAGE_KEY);
  return store;
}

async function loadSettings(): Promise<AppSettings> {
  try {
    const s = await getStore();
    const data = await s.get<Partial<AppSettings>>("settings");
    return { ...defaults, ...data };
  } catch (e) {
    console.error("[Settings] Failed to load from store:", e);
    return { ...defaults };
  }
}

async function saveSettings(settings: AppSettings): Promise<void> {
  try {
    const s = await getStore();
    await s.set("settings", settings);
    await s.save();
  } catch (e) {
    console.error("[Settings] Failed to save to store:", e);
  }
}

export const settings = $state<AppSettings>({ ...defaults });

let loaded = false;

export async function initSettings(): Promise<void> {
  if (loaded) return;
  const data = await loadSettings();
  settings.theme = data.theme;
  settings.sidebarCollapsed = data.sidebarCollapsed;
  settings.fontSize = data.fontSize;
  settings.notificationsEnabled = data.notificationsEnabled;
  applyTheme(data.theme);
  loaded = true;
}

export function applyTheme(theme: AppSettings["theme"]) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  if (theme === "dark") {
    root.classList.add("dark");
  } else if (theme === "light") {
    root.classList.remove("dark");
  } else {
    if (window.matchMedia("(prefers-color-scheme: dark)").matches) {
      root.classList.add("dark");
    } else {
      root.classList.remove("dark");
    }
  }
  // Notify components that depend on the resolved dark/light state
  window.dispatchEvent(new CustomEvent("theme-changed", { detail: { theme } }));
}

let mediaQuery: MediaQueryList | null = null;
let mediaListener: ((e: MediaQueryListEvent) => void) | null = null;

export function startThemeListener() {
  if (typeof window === "undefined" || mediaQuery) return;
  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  mediaListener = () => {
    if (settings.theme === "system") {
      applyTheme("system");
    }
  };
  mediaQuery.addEventListener("change", mediaListener);
}

export function stopThemeListener() {
  if (mediaQuery && mediaListener) {
    mediaQuery.removeEventListener("change", mediaListener);
    mediaQuery = null;
    mediaListener = null;
  }
}

export function persistSettings(s: AppSettings) {
  saveSettings(s);
}
