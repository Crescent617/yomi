import { describe, expect, test } from "vitest";

import {
  BUILTIN_THEMES,
  cloneTheme,
  hexToHslTriplet,
  hslTripletToHex,
  isHexColor,
  PALETTE_KEYS,
  parseThemeJson,
  resolveCssVars,
  resolveTheme,
  type ColorTheme,
} from "./palettes";

describe("hexToHslTriplet", () => {
  test("converts pure colors", () => {
    expect(hexToHslTriplet("#000000")).toBe("0 0% 0%");
    expect(hexToHslTriplet("#ffffff")).toBe("0 0% 100%");
    expect(hexToHslTriplet("#ff0000")).toBe("0 100% 50%");
    expect(hexToHslTriplet("#00ff00")).toBe("120 100% 50%");
    expect(hexToHslTriplet("#0000ff")).toBe("240 100% 50%");
  });

  test("converts the Zed dark background", () => {
    expect(hexToHslTriplet("#282c33")).toBe("218.2 12.1% 17.8%");
  });
});

describe("hslTripletToHex", () => {
  test("is the inverse of hexToHslTriplet for pure colors", () => {
    expect(hslTripletToHex("0 0% 0%")).toBe("#000000");
    expect(hslTripletToHex("0 0% 100%")).toBe("#ffffff");
    expect(hslTripletToHex("240 100% 50%")).toBe("#0000ff");
  });

  test("roundtrips within one channel unit", () => {
    for (const hex of ["#282c33", "#5c78e2", "#dec184", "#073642"]) {
      const roundtrip = hslTripletToHex(hexToHslTriplet(hex));
      for (let i = 0; i < 3; i++) {
        const before = parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
        const after = parseInt(roundtrip.slice(1 + i * 2, 3 + i * 2), 16);
        expect(Math.abs(before - after)).toBeLessThanOrEqual(1);
      }
    }
  });

  test("rejects invalid triplets", () => {
    expect(() => hslTripletToHex("not a color")).toThrow();
  });
});

describe("resolveCssVars", () => {
  const palette = BUILTIN_THEMES[0].dark;

  test("emits every palette var plus derived vars", () => {
    const vars = resolveCssVars(palette, true);
    for (const key of PALETTE_KEYS) {
      expect(vars[`--${key}`]).toBeDefined();
    }
    expect(vars["--overlay"]).toBeDefined();
    expect(vars["--subtle"]).toBeDefined();
  });

  test("code-bg is a full hsl() value, others are triplets", () => {
    const vars = resolveCssVars(palette, true);
    expect(vars["--code-bg"]).toMatch(/^hsl\(/);
    expect(vars["--background"]).toBe("45 8% 9.8%");
  });

  test("derives overlay alpha from the variant", () => {
    expect(resolveCssVars(palette, true)["--overlay"]).toBe(
      "hsl(0 0% 0% / 0.6)",
    );
    expect(resolveCssVars(palette, false)["--overlay"]).toBe(
      "hsl(0 0% 0% / 0.4)",
    );
  });

  test("derives subtle from muted-foreground", () => {
    const vars = resolveCssVars(palette, true);
    const expected = `hsl(${hexToHslTriplet(palette["muted-foreground"])} / 0.1)`;
    expect(vars["--subtle"]).toBe(expected);
  });
});

describe("builtin themes", () => {
  test("ship the expected set", () => {
    expect(BUILTIN_THEMES.map((t) => t.id)).toEqual([
      "yomi-ink",
      "yomi-ai",
      "zed-one",
      "github",
      "solarized",
      "nord",
      "dracula",
    ]);
    expect(BUILTIN_THEMES.every((t) => t.builtin)).toBe(true);
  });

  test("every palette entry is a valid hex color", () => {
    for (const theme of BUILTIN_THEMES) {
      for (const variant of [theme.light, theme.dark]) {
        for (const key of PALETTE_KEYS) {
          expect(
            isHexColor(variant[key]),
            `${theme.id} ${key} should be hex`,
          ).toBe(true);
        }
      }
    }
  });
});

describe("resolveTheme", () => {
  test("finds by id and falls back to the first theme", () => {
    expect(resolveTheme(BUILTIN_THEMES, "nord").id).toBe("nord");
    expect(resolveTheme(BUILTIN_THEMES, "missing").id).toBe("yomi-ink");
  });
});

describe("cloneTheme", () => {
  test("deep-copies palettes and marks as custom", () => {
    const base = BUILTIN_THEMES[0];
    const copy: ColorTheme = cloneTheme(base, "custom-x", "Mine");
    expect(copy.builtin).toBe(false);
    expect(copy.light).toEqual(base.light);
    expect(copy.light).not.toBe(base.light);
    copy.light.background = "#000000";
    expect(base.light.background).not.toBe("#000000");
  });
});

describe("parseThemeJson", () => {
  const validDoc = () =>
    JSON.stringify({
      name: "Mine",
      light: BUILTIN_THEMES[0].light,
      dark: BUILTIN_THEMES[0].dark,
      extra: "dropped",
    });

  test("parses a full theme document", () => {
    const result = parseThemeJson(validDoc());
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.name).toBe("Mine");
      expect(result.light).toEqual(BUILTIN_THEMES[0].light);
      expect(result.dark).toEqual(BUILTIN_THEMES[0].dark);
    }
  });

  test("rejects invalid JSON", () => {
    const result = parseThemeJson("{not json");
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatch(/Invalid JSON/);
  });

  test("rejects a missing or empty name", () => {
    const doc = JSON.parse(validDoc());
    delete doc.name;
    expect(parseThemeJson(JSON.stringify(doc)).ok).toBe(false);
    doc.name = "   ";
    expect(parseThemeJson(JSON.stringify(doc)).ok).toBe(false);
  });

  test("rejects palettes with missing keys", () => {
    const doc = JSON.parse(validDoc());
    delete doc.light.primary;
    const result = parseThemeJson(JSON.stringify(doc));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatch(/light\.primary/);
  });

  test("rejects non-hex colors", () => {
    const doc = JSON.parse(validDoc());
    doc.dark.border = "red";
    const result = parseThemeJson(JSON.stringify(doc));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatch(/dark\.border/);
  });
});
