type Rgb = { r: number; g: number; b: number };

function cssColor(variable: string): string {
  const probe = document.createElement("span");
  probe.style.color = `hsl(var(${variable}))`;
  probe.style.display = "none";
  document.body.appendChild(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return rgbToHex(parseRgb(value));
}

function parseRgb(value: string): Rgb {
  const channels = value
    .match(/[\d.]+/g)
    ?.slice(0, 3)
    .map(Number);
  if (!channels || channels.length < 3) return { r: 0, g: 0, b: 0 };
  return { r: channels[0], g: channels[1], b: channels[2] };
}

function rgbToHex({ r, g, b }: Rgb): string {
  const channel = (value: number) =>
    Math.round(Math.max(0, Math.min(255, value)))
      .toString(16)
      .padStart(2, "0");
  return `#${channel(r)}${channel(g)}${channel(b)}`;
}

function hexToRgb(hex: string): Rgb {
  return {
    r: Number.parseInt(hex.slice(1, 3), 16),
    g: Number.parseInt(hex.slice(3, 5), 16),
    b: Number.parseInt(hex.slice(5, 7), 16),
  };
}

/** Mix `foreground` into `background` by `amount`. */
function mix(background: string, foreground: string, amount: number): string {
  const bg = hexToRgb(background);
  const fg = hexToRgb(foreground);
  return rgbToHex({
    r: bg.r + (fg.r - bg.r) * amount,
    g: bg.g + (fg.g - bg.g) * amount,
    b: bg.b + (fg.b - bg.b) * amount,
  });
}

export type MermaidTheme = Record<string, string | boolean>;

/** Build a Mermaid base theme directly from the active semantic CSS tokens. */
export function mermaidTheme(): MermaidTheme {
  const darkMode = document.documentElement.classList.contains("dark");
  const background = cssColor("--background");
  const foreground = cssColor("--foreground");
  const card = cssColor("--card");
  const secondary = cssColor("--secondary");
  const primary = cssColor("--primary");
  const border = cssColor("--border");
  const mutedForeground = cssColor("--muted-foreground");
  const warning = cssColor("--warning");
  const error = cssColor("--error");
  const primaryTint = mix(card, primary, darkMode ? 0.1 : 0.1);
  const secondaryTint = mix(card, secondary, darkMode ? 0.38 : 0.8);
  const tertiaryTint = darkMode ? mix(card, background, 0.28) : card;
  const labelBackground = mix(background, card, 0.65);
  const categoryColors = darkMode
    ? [
        mix(card, primary, 0.12),
        mix(card, warning, 0.1),
        mix(card, error, 0.09),
        mix(card, secondary, 0.45),
      ]
    : [primaryTint, secondaryTint, card, mix(card, warning, 0.08)];

  return {
    darkMode,
    background,
    fontFamily: '"Inter", system-ui, sans-serif',
    fontSize: "14px",

    primaryColor: primaryTint,
    primaryTextColor: foreground,
    primaryBorderColor: primary,
    secondaryColor: secondaryTint,
    secondaryTextColor: foreground,
    secondaryBorderColor: border,
    tertiaryColor: tertiaryTint,
    tertiaryTextColor: foreground,
    tertiaryBorderColor: border,
    textColor: foreground,
    titleColor: foreground,
    lineColor: mutedForeground,

    cScale0: categoryColors[0],
    cScale1: categoryColors[1],
    cScale2: categoryColors[2],
    cScale3: categoryColors[3],
    cScale4: categoryColors[0],
    cScale5: categoryColors[1],
    cScale6: categoryColors[2],
    cScale7: categoryColors[3],
    cScale8: categoryColors[0],
    cScale9: categoryColors[1],
    cScale10: categoryColors[2],
    cScale11: categoryColors[3],
    cScaleLabel0: foreground,
    cScaleLabel1: foreground,
    cScaleLabel2: foreground,
    cScaleLabel3: foreground,
    cScaleLabel4: foreground,
    cScaleLabel5: foreground,
    cScaleLabel6: foreground,
    cScaleLabel7: foreground,
    cScaleLabel8: foreground,
    cScaleLabel9: foreground,
    cScaleLabel10: foreground,
    cScaleLabel11: foreground,
    cScaleInv0: foreground,
    cScaleInv1: foreground,
    cScaleInv2: foreground,
    cScaleInv3: foreground,
    cScaleInv4: foreground,
    cScaleInv5: foreground,
    cScaleInv6: foreground,
    cScaleInv7: foreground,
    cScaleInv8: foreground,
    cScaleInv9: foreground,
    cScaleInv10: foreground,
    cScaleInv11: foreground,

    mainBkg: primaryTint,
    nodeBkg: primaryTint,
    nodeBorder: primary,
    nodeTextColor: foreground,
    clusterBkg: mix(background, secondary, darkMode ? 0.58 : 0.72),
    clusterBorder: border,
    clusterTextColor: foreground,
    edgeLabelBackground: labelBackground,
    labelBackground,

    noteBkgColor: mix(card, warning, darkMode ? 0.2 : 0.12),
    noteTextColor: foreground,
    noteBorderColor: warning,
    errorBkgColor: mix(card, error, darkMode ? 0.2 : 0.1),
    errorTextColor: foreground,

    actorBkg: primaryTint,
    actorBorder: primary,
    actorTextColor: foreground,
    actorLineColor: border,
    signalColor: mutedForeground,
    signalTextColor: foreground,
    labelBoxBkgColor: card,
    labelBoxBorderColor: border,
    labelTextColor: foreground,
    loopTextColor: foreground,
    activationBkgColor: secondaryTint,
    activationBorderColor: primary,
    sequenceNumberColor: foreground,

    classText: foreground,
    stateLabelColor: foreground,
    stateBkg: primaryTint,
    stateBorder: primary,
    transitionColor: mutedForeground,
    transitionLabelColor: foreground,
    specialStateColor: primary,

    attributeBackgroundColorOdd: card,
    attributeBackgroundColorEven: secondaryTint,
    entityBkg: primaryTint,
    entityBorder: primary,
    relationLabelBackground: labelBackground,
    relationLabelColor: foreground,

    taskBkgColor: primaryTint,
    taskTextColor: foreground,
    taskTextDarkColor: foreground,
    taskTextLightColor: foreground,
    taskTextOutsideColor: foreground,
    activeTaskBkgColor: secondaryTint,
    activeTaskBorderColor: primary,
    doneTaskBkgColor: secondaryTint,
    doneTaskBorderColor: border,
    gridColor: border,
    todayLineColor: primary,

    sectionBkgColor: secondaryTint,
    sectionBkgColor2: card,
    sectionBkgColor3: secondaryTint,
    sectionBkgColor4: card,
    sectionBkgColor5: secondaryTint,
    sectionBkgColor6: card,
    sectionBkgColor7: secondaryTint,
    sectionBkgColor8: card,
    sectionBkgColor9: secondaryTint,
    sectionBkgColor10: card,
    sectionBkgColor11: secondaryTint,
    sectionBkgColor12: card,
    sectionBkgColor13: secondaryTint,
    sectionBkgColor14: card,
    sectionBkgColor15: secondaryTint,
    sectionBkgColor16: card,
    sectionBkgColor17: secondaryTint,
    sectionBkgColor18: card,
    sectionBkgColor19: secondaryTint,
    sectionBkgColor20: card,
    sectionBkgColor21: secondaryTint,
    sectionBkgColor22: card,
    sectionBkgColor23: secondaryTint,
    sectionBkgColor24: card,
    sectionBkgColor25: secondaryTint,
    sectionBkgColor26: card,
    sectionBkgColor27: secondaryTint,
    sectionBkgColor28: card,
    sectionBkgColor29: secondaryTint,
    sectionBkgColor30: card,
    sectionBkgColor31: secondaryTint,
    sectionBkgColor32: card,
  };
}
