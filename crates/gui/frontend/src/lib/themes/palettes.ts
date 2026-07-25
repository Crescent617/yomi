// Theme palette data and pure color utilities.
//
// A `ThemePalette` holds hex colors (#rrggbb) for every themeable variable.
// `resolveCssVars` converts a palette into the final CSS custom-property map
// consumed by app.css (@theme maps `hsl(var(--background))` etc.):
//   - most vars become HSL channel triplets ("218.2 12.1% 17.8%")
//   - --code-bg becomes a full hsl() value
//   - --overlay / --subtle are derived (not user-editable)
//
// This module is intentionally free of runtime imports so it stays unit-testable.

/** Every themeable variable. Values are hex colors (#rrggbb). */
export interface ThemePalette {
  background: string;
  foreground: string;
  card: string;
  "card-foreground": string;
  popover: string;
  "popover-foreground": string;
  primary: string;
  "primary-foreground": string;
  secondary: string;
  "secondary-foreground": string;
  muted: string;
  "muted-foreground": string;
  accent: string;
  "accent-foreground": string;
  destructive: string;
  "destructive-foreground": string;
  border: string;
  input: string;
  ring: string;
  "code-bg": string;
  success: string;
  "success-foreground": string;
  warning: string;
  "warning-foreground": string;
  error: string;
  "error-foreground": string;
  info: string;
  "info-foreground": string;
}

export interface ColorTheme {
  id: string;
  name: string;
  builtin: boolean;
  light: ThemePalette;
  dark: ThemePalette;
}

/** Palette keys rendered in the editor, grouped for humans. */
export const PALETTE_GROUPS: Array<{
  id: string;
  label: string;
  keys: Array<keyof ThemePalette>;
}> = [
  {
    id: "surface",
    label: "Surface",
    keys: [
      "background",
      "card",
      "popover",
      "secondary",
      "muted",
      "accent",
      "code-bg",
    ],
  },
  {
    id: "text",
    label: "Text",
    keys: [
      "foreground",
      "card-foreground",
      "popover-foreground",
      "secondary-foreground",
      "muted-foreground",
      "accent-foreground",
    ],
  },
  {
    id: "accent",
    label: "Accent",
    keys: ["primary", "primary-foreground", "ring"],
  },
  {
    id: "status",
    label: "Status",
    keys: [
      "success",
      "success-foreground",
      "warning",
      "warning-foreground",
      "error",
      "error-foreground",
      "info",
      "info-foreground",
      "destructive",
      "destructive-foreground",
    ],
  },
  {
    id: "border",
    label: "Border",
    keys: ["border", "input"],
  },
];

export const PALETTE_KEYS: Array<keyof ThemePalette> = PALETTE_GROUPS.flatMap(
  (group) => group.keys,
);

const HEX_RE = /^#[0-9a-fA-F]{6}$/;

export function isHexColor(value: string): boolean {
  return HEX_RE.test(value);
}

function fmtChannel(value: number): string {
  return String(Number(value.toFixed(1)));
}

/** "#282c33" → "218.2 12.1% 17.8%" */
export function hexToHslTriplet(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) {
    return `0 0% ${fmtChannel(l * 100)}%`;
  }
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === r) {
    h = (g - b) / d + (g < b ? 6 : 0);
  } else if (max === g) {
    h = (b - r) / d + 2;
  } else {
    h = (r - g) / d + 4;
  }
  h *= 60;
  return `${fmtChannel(h)} ${fmtChannel(s * 100)}% ${fmtChannel(l * 100)}%`;
}

function hueToRgb(p: number, q: number, t: number): number {
  let tt = t;
  if (tt < 0) tt += 1;
  if (tt > 1) tt -= 1;
  if (tt < 1 / 6) return p + (q - p) * 6 * tt;
  if (tt < 1 / 2) return q;
  if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
  return p;
}

/** "218.2 12.1% 17.8%" → "#282c33" (inverse of hexToHslTriplet) */
export function hslTripletToHex(triplet: string): string {
  const match = triplet.match(/^([\d.]+)\s+([\d.]+)%\s+([\d.]+)%$/);
  if (!match) throw new Error(`Invalid HSL triplet: ${triplet}`);
  const h = Number(match[1]) / 360;
  const s = Number(match[2]) / 100;
  const l = Number(match[3]) / 100;
  const to255 = (v: number) =>
    Math.round(v * 255)
      .toString(16)
      .padStart(2, "0");
  if (s === 0) {
    const gray = to255(l);
    return `#${gray}${gray}${gray}`;
  }
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  return `#${to255(hueToRgb(p, q, h + 1 / 3))}${to255(hueToRgb(p, q, h))}${to255(
    hueToRgb(p, q, h - 1 / 3),
  )}`;
}

/**
 * Convert a palette into the final CSS custom-property map.
 * --overlay / --subtle are derived: overlay from the variant kind, subtle
 * from muted-foreground.
 */
export function resolveCssVars(
  palette: ThemePalette,
  dark: boolean,
): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const key of PALETTE_KEYS) {
    const triplet = hexToHslTriplet(palette[key]);
    if (key === "code-bg") {
      vars["--code-bg"] = `hsl(${triplet})`;
    } else {
      vars[`--${key}`] = triplet;
    }
  }
  vars["--overlay"] = `hsl(0 0% 0% / ${dark ? 0.6 : 0.4})`;
  vars["--subtle"] =
    `hsl(${hexToHslTriplet(palette["muted-foreground"])} / 0.1)`;
  return vars;
}

/** Find a theme by id, falling back to the first theme (Zed One). */
export function resolveTheme(themes: ColorTheme[], id: string): ColorTheme {
  return themes.find((theme) => theme.id === id) ?? themes[0];
}

/**
 * Return an error describing the first invalid palette key, or null when
 * every key is present and a valid hex color.
 */
export function validatePalette(
  value: unknown,
  label = "palette",
): string | null {
  if (typeof value !== "object" || value === null) {
    return `"${label}" must be an object`;
  }
  const record = value as Record<string, unknown>;
  for (const key of PALETTE_KEYS) {
    const color = record[key];
    if (typeof color !== "string" || !isHexColor(color)) {
      return `"${label}.${key}" must be a hex color like #282a36`;
    }
  }
  return null;
}

/** Copy only the known palette keys (assumes validatePalette passed). */
export function pickPalette(value: unknown): ThemePalette {
  const record = value as Record<string, string>;
  const palette = {} as Record<keyof ThemePalette, string>;
  for (const key of PALETTE_KEYS) {
    palette[key] = record[key];
  }
  return palette;
}

export type ParseThemeResult =
  | { ok: true; name: string; light: ThemePalette; dark: ThemePalette }
  | { ok: false; error: string };

/**
 * Parse a theme JSON document of the form `{ name, light, dark }`.
 * Unknown keys are dropped; every palette key must be present and hex.
 */
export function parseThemeJson(text: string): ParseThemeResult {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    return {
      ok: false,
      error: `Invalid JSON: ${error instanceof Error ? error.message : error}`,
    };
  }
  if (typeof value !== "object" || value === null) {
    return { ok: false, error: "Expected a JSON object" };
  }
  const record = value as Record<string, unknown>;
  if (typeof record.name !== "string" || record.name.trim().length === 0) {
    return { ok: false, error: '"name" must be a non-empty string' };
  }
  for (const variant of ["light", "dark"] as const) {
    const problem = validatePalette(record[variant], variant);
    if (problem) return { ok: false, error: problem };
  }
  return {
    ok: true,
    name: record.name.trim(),
    light: pickPalette(record.light),
    dark: pickPalette(record.dark),
  };
}

/** Deep-clone a theme under a new id/name (for "clone into custom"). */
export function cloneTheme(
  base: ColorTheme,
  id: string,
  name: string,
): ColorTheme {
  return {
    id,
    name,
    builtin: false,
    light: { ...base.light },
    dark: { ...base.dark },
  };
}

export const DEFAULT_THEME_ID = "zed-one";

export const BUILTIN_THEMES: ColorTheme[] = [
  {
    id: DEFAULT_THEME_ID,
    name: "Zed One",
    builtin: true,
    // Mirrors the original app.css palettes.
    light: {
      background: "#fafafa",
      foreground: "#242529",
      card: "#f2f2f3",
      "card-foreground": "#242529",
      popover: "#ebebec",
      "popover-foreground": "#242529",
      primary: "#5c78e2",
      "primary-foreground": "#fafafa",
      secondary: "#ebebec",
      "secondary-foreground": "#242529",
      muted: "#ebebec",
      "muted-foreground": "#58585a",
      accent: "#dfdfe0",
      "accent-foreground": "#242529",
      destructive: "#d36151",
      "destructive-foreground": "#fafafa",
      border: "#c9c9ca",
      input: "#c9c9ca",
      ring: "#7d82e8",
      "code-bg": "#ebebec",
      success: "#669f59",
      "success-foreground": "#fafafa",
      warning: "#a48819",
      "warning-foreground": "#fafafa",
      error: "#d36151",
      "error-foreground": "#fafafa",
      info: "#5c78e2",
      "info-foreground": "#fafafa",
    },
    dark: {
      background: "#282c33",
      foreground: "#dce0e5",
      card: "#2f343e",
      "card-foreground": "#dce0e5",
      popover: "#2f343e",
      "popover-foreground": "#dce0e5",
      primary: "#74ade8",
      "primary-foreground": "#282c33",
      secondary: "#2e343e",
      "secondary-foreground": "#dce0e5",
      muted: "#2e343e",
      "muted-foreground": "#a9afbc",
      accent: "#363c46",
      "accent-foreground": "#dce0e5",
      destructive: "#d07277",
      "destructive-foreground": "#282c33",
      border: "#464b57",
      input: "#464b57",
      ring: "#47679e",
      "code-bg": "#2f343e",
      success: "#a1c181",
      "success-foreground": "#282c33",
      warning: "#dec184",
      "warning-foreground": "#282c33",
      error: "#d07277",
      "error-foreground": "#282c33",
      info: "#74ade8",
      "info-foreground": "#282c33",
    },
  },
  {
    id: "github",
    name: "GitHub",
    builtin: true,
    light: {
      background: "#ffffff",
      foreground: "#1f2328",
      card: "#f6f8fa",
      "card-foreground": "#1f2328",
      popover: "#f6f8fa",
      "popover-foreground": "#1f2328",
      primary: "#0969da",
      "primary-foreground": "#ffffff",
      secondary: "#eaeef2",
      "secondary-foreground": "#1f2328",
      muted: "#eaeef2",
      "muted-foreground": "#59636e",
      accent: "#e2e6ea",
      "accent-foreground": "#1f2328",
      destructive: "#d1242f",
      "destructive-foreground": "#ffffff",
      border: "#d1d9e0",
      input: "#d1d9e0",
      ring: "#0969da",
      "code-bg": "#eaeef2",
      success: "#1a7f37",
      "success-foreground": "#ffffff",
      warning: "#9a6700",
      "warning-foreground": "#ffffff",
      error: "#d1242f",
      "error-foreground": "#ffffff",
      info: "#0969da",
      "info-foreground": "#ffffff",
    },
    dark: {
      background: "#0d1117",
      foreground: "#e6edf3",
      card: "#161b22",
      "card-foreground": "#e6edf3",
      popover: "#21262d",
      "popover-foreground": "#e6edf3",
      primary: "#4493f8",
      "primary-foreground": "#0d1117",
      secondary: "#21262d",
      "secondary-foreground": "#e6edf3",
      muted: "#21262d",
      "muted-foreground": "#9198a1",
      accent: "#262c36",
      "accent-foreground": "#e6edf3",
      destructive: "#f85149",
      "destructive-foreground": "#0d1117",
      border: "#3d444d",
      input: "#3d444d",
      ring: "#1f6feb",
      "code-bg": "#161b22",
      success: "#3fb950",
      "success-foreground": "#0d1117",
      warning: "#d29922",
      "warning-foreground": "#0d1117",
      error: "#f85149",
      "error-foreground": "#0d1117",
      info: "#4493f8",
      "info-foreground": "#0d1117",
    },
  },
  {
    id: "solarized",
    name: "Solarized",
    builtin: true,
    light: {
      background: "#fdf6e3",
      foreground: "#657b83",
      card: "#eee8d5",
      "card-foreground": "#657b83",
      popover: "#eee8d5",
      "popover-foreground": "#657b83",
      primary: "#268bd2",
      "primary-foreground": "#fdf6e3",
      secondary: "#eee8d5",
      "secondary-foreground": "#657b83",
      muted: "#eee8d5",
      "muted-foreground": "#93a1a1",
      accent: "#e3dcc7",
      "accent-foreground": "#586e75",
      destructive: "#dc322f",
      "destructive-foreground": "#fdf6e3",
      border: "#d8d0ba",
      input: "#d8d0ba",
      ring: "#268bd2",
      "code-bg": "#eee8d5",
      success: "#859900",
      "success-foreground": "#fdf6e3",
      warning: "#b58900",
      "warning-foreground": "#fdf6e3",
      error: "#dc322f",
      "error-foreground": "#fdf6e3",
      info: "#2aa198",
      "info-foreground": "#fdf6e3",
    },
    dark: {
      background: "#002b36",
      foreground: "#839496",
      card: "#073642",
      "card-foreground": "#839496",
      popover: "#073642",
      "popover-foreground": "#839496",
      primary: "#268bd2",
      "primary-foreground": "#002b36",
      secondary: "#073642",
      "secondary-foreground": "#839496",
      muted: "#073642",
      "muted-foreground": "#84969c",
      accent: "#0c4a5a",
      "accent-foreground": "#93a1a1",
      destructive: "#dc322f",
      "destructive-foreground": "#002b36",
      border: "#14505f",
      input: "#14505f",
      ring: "#268bd2",
      "code-bg": "#073642",
      success: "#859900",
      "success-foreground": "#002b36",
      warning: "#b58900",
      "warning-foreground": "#002b36",
      error: "#dc322f",
      "error-foreground": "#002b36",
      info: "#2aa198",
      "info-foreground": "#002b36",
    },
  },
  {
    id: "nord",
    name: "Nord",
    builtin: true,
    // Nord has no official light variant; "Snow Storm" is a derived one.
    light: {
      background: "#eceff4",
      foreground: "#2e3440",
      card: "#e5e9f0",
      "card-foreground": "#2e3440",
      popover: "#e5e9f0",
      "popover-foreground": "#2e3440",
      primary: "#5e81ac",
      "primary-foreground": "#eceff4",
      secondary: "#d8dee9",
      "secondary-foreground": "#2e3440",
      muted: "#d8dee9",
      "muted-foreground": "#4c566a",
      accent: "#cfd6e2",
      "accent-foreground": "#2e3440",
      destructive: "#bf616a",
      "destructive-foreground": "#eceff4",
      border: "#b8c2d4",
      input: "#b8c2d4",
      ring: "#81a1c1",
      "code-bg": "#d8dee9",
      success: "#6f8f57",
      "success-foreground": "#eceff4",
      warning: "#b48d3c",
      "warning-foreground": "#eceff4",
      error: "#bf616a",
      "error-foreground": "#eceff4",
      info: "#5e81ac",
      "info-foreground": "#eceff4",
    },
    dark: {
      background: "#2e3440",
      foreground: "#d8dee9",
      card: "#3b4252",
      "card-foreground": "#d8dee9",
      popover: "#3b4252",
      "popover-foreground": "#d8dee9",
      primary: "#88c0d0",
      "primary-foreground": "#2e3440",
      secondary: "#434c5e",
      "secondary-foreground": "#d8dee9",
      muted: "#434c5e",
      "muted-foreground": "#9ca8bd",
      accent: "#4c566a",
      "accent-foreground": "#eceff4",
      destructive: "#bf616a",
      "destructive-foreground": "#2e3440",
      border: "#4c566a",
      input: "#4c566a",
      ring: "#81a1c1",
      "code-bg": "#3b4252",
      success: "#a3be8c",
      "success-foreground": "#2e3440",
      warning: "#ebcb8b",
      "warning-foreground": "#2e3440",
      error: "#bf616a",
      "error-foreground": "#2e3440",
      info: "#81a1c1",
      "info-foreground": "#2e3440",
    },
  },
  {
    id: "dracula",
    name: "Dracula",
    builtin: true,
    // Dracula is dark-native; the light variant is an Alucard-style derivation.
    light: {
      background: "#f8f8f2",
      foreground: "#282a36",
      card: "#efeee4",
      "card-foreground": "#282a36",
      popover: "#efeee4",
      "popover-foreground": "#282a36",
      primary: "#6c4fc0",
      "primary-foreground": "#f8f8f2",
      secondary: "#e6e4d8",
      "secondary-foreground": "#282a36",
      muted: "#e6e4d8",
      "muted-foreground": "#6272a4",
      accent: "#dcdacc",
      "accent-foreground": "#282a36",
      destructive: "#d63030",
      "destructive-foreground": "#f8f8f2",
      border: "#d0cec0",
      input: "#d0cec0",
      ring: "#6c4fc0",
      "code-bg": "#efeee4",
      success: "#3f9e57",
      "success-foreground": "#f8f8f2",
      warning: "#a87a1f",
      "warning-foreground": "#f8f8f2",
      error: "#d63030",
      "error-foreground": "#f8f8f2",
      info: "#0e8fa8",
      "info-foreground": "#f8f8f2",
    },
    dark: {
      background: "#282a36",
      foreground: "#f8f8f2",
      card: "#323442",
      "card-foreground": "#f8f8f2",
      popover: "#323442",
      "popover-foreground": "#323442",
      primary: "#bd93f9",
      "primary-foreground": "#282a36",
      secondary: "#3a3d50",
      "secondary-foreground": "#f8f8f2",
      muted: "#3a3d50",
      "muted-foreground": "#8b98c9",
      accent: "#44475a",
      "accent-foreground": "#f8f8f2",
      destructive: "#ff5555",
      "destructive-foreground": "#282a36",
      border: "#44475a",
      input: "#44475a",
      ring: "#bd93f9",
      "code-bg": "#323442",
      success: "#50fa7b",
      "success-foreground": "#282a36",
      warning: "#f1fa8c",
      "warning-foreground": "#282a36",
      error: "#ff5555",
      "error-foreground": "#282a36",
      info: "#8be9fd",
      "info-foreground": "#282a36",
    },
  },
];
