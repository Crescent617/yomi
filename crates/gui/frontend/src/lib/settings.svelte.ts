export interface AppSettings {
  theme: "light" | "dark" | "system";
  sidebarCollapsed: boolean;
  fontSize: "sm" | "base" | "lg";
  notificationsEnabled: boolean;
}

const STORAGE_KEY = "yomi-gui-settings";

const defaults: AppSettings = {
  theme: "system",
  sidebarCollapsed: false,
  fontSize: "base",
  notificationsEnabled: true,
};

function load(): AppSettings {
  if (typeof window === "undefined") return { ...defaults };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return { ...defaults, ...parsed };
    }
  } catch {
    // ignore
  }
  return { ...defaults };
}

function save(settings: AppSettings) {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // ignore
  }
}

export const settings = $state<AppSettings>(load());

// Auto-save on mutation (deep watch would be complex, so we expose save explicitly)
export { save as persistSettings, defaults as defaultSettings };

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
