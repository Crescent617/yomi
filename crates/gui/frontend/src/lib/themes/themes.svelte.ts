// Color theme runtime: custom-theme persistence, theme resolution, and
// application of palettes to the document.
//
// Custom themes persist in the shared settings store under their own key;
// the *resolved* CSS variables of the active theme are mirrored to
// localStorage so app.html can apply them pre-paint (anti-FOUC).
import { getSettingsStore } from "../settings-store";
import {
  BUILTIN_THEMES,
  DEFAULT_THEME_ID,
  cloneTheme,
  pickPalette,
  resolveCssVars,
  resolveTheme,
  validatePalette,
  type ColorTheme,
  type ThemePalette,
} from "./palettes";

const CUSTOM_THEMES_KEY = "custom_themes";
const ACTIVE_THEME_CACHE_KEY = "yomi-gui-active-theme";

export const customThemes = $state<ColorTheme[]>([]);

let initPromise: Promise<void> | null = null;

export function getAllThemes(): ColorTheme[] {
  return [...BUILTIN_THEMES, ...customThemes];
}

export function getThemeById(id: string): ColorTheme {
  return resolveTheme(getAllThemes(), id);
}

function isDarkMode(): boolean {
  return document.documentElement.classList.contains("dark");
}

/** Apply a theme variant's palette to the document and mirror it for app.html. */
export function applyColorTheme(
  theme: ColorTheme,
  dark = isDarkMode(),
  mirror = true,
): void {
  if (typeof document === "undefined") return;
  const vars = resolveCssVars(dark ? theme.dark : theme.light, dark);
  const style = document.documentElement.style;
  for (const [name, value] of Object.entries(vars)) {
    style.setProperty(name, value);
  }
  if (!mirror) return;
  try {
    localStorage.setItem(
      ACTIVE_THEME_CACHE_KEY,
      JSON.stringify({ dark, vars }),
    );
  } catch {
    // localStorage unavailable — theme still applies for this session.
  }
}

/** Apply the palette for a theme id (unknown ids fall back to the default). */
export function applyColorThemeById(id: string, dark = isDarkMode()): void {
  applyColorTheme(getThemeById(id), dark);
}

function sanitizePalette(value: unknown): ThemePalette | null {
  return validatePalette(value) === null ? pickPalette(value) : null;
}

function sanitizeCustomTheme(value: unknown): ColorTheme | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  if (
    typeof record.id !== "string" ||
    typeof record.name !== "string" ||
    record.builtin === true
  ) {
    return null;
  }
  const light = sanitizePalette(record.light);
  const dark = sanitizePalette(record.dark);
  if (!light || !dark) return null;
  return { id: record.id, name: record.name, builtin: false, light, dark };
}

export function initCustomThemes(): Promise<void> {
  if (initPromise) return initPromise;
  initPromise = (async () => {
    try {
      const store = await getSettingsStore();
      const stored = await store.get<unknown[]>(CUSTOM_THEMES_KEY);
      if (!Array.isArray(stored)) return;
      const themes = stored
        .map(sanitizeCustomTheme)
        .filter((theme): theme is ColorTheme => theme !== null);
      customThemes.splice(0, customThemes.length, ...themes);
    } catch (error) {
      console.error("[Themes] Failed to load custom themes:", error);
    }
  })();
  return initPromise;
}

async function persistCustomThemes(): Promise<void> {
  const store = await getSettingsStore();
  await store.set(CUSTOM_THEMES_KEY, $state.snapshot(customThemes));
  await store.save();
}

/** Insert or replace a custom theme, then persist. */
export async function upsertCustomTheme(theme: ColorTheme): Promise<void> {
  const index = customThemes.findIndex((t) => t.id === theme.id);
  const copy: ColorTheme = {
    ...theme,
    builtin: false,
    light: { ...theme.light },
    dark: { ...theme.dark },
  };
  if (index >= 0) {
    customThemes[index] = copy;
  } else {
    customThemes.push(copy);
  }
  await persistCustomThemes();
}

export async function deleteCustomTheme(id: string): Promise<void> {
  const index = customThemes.findIndex((t) => t.id === id);
  if (index < 0) return;
  customThemes.splice(index, 1);
  await persistCustomThemes();
}

/** Clone a theme (builtin or custom) into a new editable custom theme. */
export function newCustomThemeDraft(base: ColorTheme): ColorTheme {
  const id =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? `custom-${crypto.randomUUID()}`
      : `custom-${Date.now()}`;
  const name = base.builtin ? `Custom ${base.name}` : `${base.name} copy`;
  return cloneTheme(base, id, name);
}

export { DEFAULT_THEME_ID };
