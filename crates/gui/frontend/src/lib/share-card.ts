/**
 * Share card renderer: draws an assistant answer as a branded PNG card
 * using an offscreen canvas. Markdown is rendered with real styling
 * (headings, bold/italic/code, lists, quotes, code blocks). Theme colors
 * are read from the app's CSS variables so the card matches light/dark
 * mode. Card width is configurable.
 */

import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import {
  isCjk,
  parseMarkdown,
  type Block,
  type InlineRun,
  type InlineStyle,
} from "./share-markdown";

const SCALE = 2;
const DEFAULT_CARD_WIDTH = 720;
export const MIN_CARD_WIDTH = 480;
export const MAX_CARD_WIDTH = 1080;
const CARD_PAD = 48;
const OUTER_PAD = 32;
const CARD_RADIUS = 20;
const LOGO_SIZE = 40;
const BODY_SIZE = 17;
const BODY_LH = 30;
const CODE_SIZE = 14;
const CODE_LH = 24;
const CODE_PAD_X = 14;
const CODE_PAD_Y = 10;
const QUOTE_INDENT = 16;
const LIST_INDENT = 26;
const TABLE_SIZE = 15;
const TABLE_LH = 26;
const TABLE_COL_GAP = 24;
const TABLE_ROW_GAP = 6;
const TABLE_MIN_COL_W = 48;
const MAX_BODY_HEIGHT = 1500;
const MONO_FONT = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
const HEADING = {
  1: { size: 26, lh: 40, before: 20 },
  2: { size: 22, lh: 34, before: 18 },
  3: { size: 19, lh: 30, before: 14 },
} as const;

export interface ShareCardInput {
  /** Markdown source of the answer */
  content: string;
  sessionTitle?: string;
  date: Date;
  /** Card width in px (unscaled), clamped to [MIN_CARD_WIDTH, MAX_CARD_WIDTH]. */
  width?: number;
}

interface ThemeColors {
  background: string;
  card: string;
  foreground: string;
  mutedForeground: string;
  border: string;
  primary: string;
  codeBg: string;
}

/** Read a shadcn-style HSL channel variable (e.g. "210 40% 98%") as a CSS color. */
function cssHslVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return value ? `hsl(${value})` : fallback;
}

/** Read a CSS variable that already holds a complete color value. */
function cssColorVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return value || fallback;
}

function themeColors(): ThemeColors {
  return {
    background: cssHslVar("--background", "#ffffff"),
    card: cssHslVar("--card", "#f8f9fa"),
    foreground: cssHslVar("--foreground", "#1a1d21"),
    mutedForeground: cssHslVar("--muted-foreground", "#6b7280"),
    border: cssHslVar("--border", "#e5e7eb"),
    primary: cssHslVar("--primary", "#3b82f6"),
    codeBg: cssColorVar("--code-bg", "#ebebec"),
  };
}

function roundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function loadImage(src: string): Promise<HTMLImageElement | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => resolve(null);
    img.src = src;
  });
}

function formatCardDate(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ` +
    `${pad(date.getHours())}:${pad(date.getMinutes())}`
  );
}

// ── Styled text layout ──

interface Token {
  text: string;
  style: InlineStyle;
}

type Line = Token[];

type Measure = (text: string, style: InlineStyle) => number;

/** Split runs into breakable tokens: words, single CJK chars, and spaces. */
function tokenize(runs: InlineRun[]): Token[] {
  const tokens: Token[] = [];
  for (const run of runs) {
    let buf = "";
    const pushBuf = () => {
      if (buf) {
        tokens.push({ text: buf, style: run.style });
        buf = "";
      }
    };
    for (const ch of run.text) {
      if (ch === " " || ch === "\t") {
        pushBuf();
        tokens.push({ text: " ", style: run.style });
      } else if (isCjk(ch)) {
        pushBuf();
        tokens.push({ text: ch, style: run.style });
      } else {
        buf += ch;
      }
    }
    pushBuf();
  }
  return tokens;
}

function lineWidth(line: Line, measure: Measure): number {
  let w = 0;
  for (const t of line) w += measure(t.text, t.style);
  return w;
}

/** Split an over-wide token into full lines plus a remainder line. */
function splitToken(tok: Token, maxWidth: number, measure: Measure): Line[] {
  const lines: Line[] = [];
  let rest = tok.text;
  while (rest.length > 1 && measure(rest, tok.style) > maxWidth) {
    let lo = 1;
    let hi = rest.length;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      if (measure(rest.slice(0, mid), tok.style) <= maxWidth) lo = mid;
      else hi = mid - 1;
    }
    lines.push([{ text: rest.slice(0, lo), style: tok.style }]);
    rest = rest.slice(lo);
  }
  if (rest) lines.push([{ text: rest, style: tok.style }]);
  return lines;
}

/** Greedy word wrap for styled runs (CJK-safe, style-aware measuring). */
function wrapTokens(
  runs: InlineRun[],
  maxWidth: number,
  measure: Measure,
  preserveLeadingSpace = false,
): Line[] {
  const tokens = tokenize(runs);
  const lines: Line[] = [];
  let cur: Line = [];
  let curWidth = 0;
  let pendingSpace: Token | null = null;

  const flushLine = () => {
    if (cur.length > 0) lines.push(cur);
    cur = [];
    curWidth = 0;
    pendingSpace = null;
  };

  for (const tok of tokens) {
    if (tok.text === " ") {
      // Code blocks keep indentation verbatim: spaces act as normal tokens.
      if (preserveLeadingSpace) {
        const w = measure(" ", tok.style);
        if (curWidth + w <= maxWidth) {
          cur.push(tok);
          curWidth += w;
        } else {
          flushLine();
        }
        continue;
      }
      // Spaces only render between tokens on the same line.
      if (cur.length > 0) pendingSpace = tok;
      continue;
    }
    const spaceW = pendingSpace ? measure(" ", pendingSpace.style) : 0;
    const tokW = measure(tok.text, tok.style);
    if (curWidth + spaceW + tokW <= maxWidth) {
      if (pendingSpace) {
        cur.push(pendingSpace);
        curWidth += spaceW;
        pendingSpace = null;
      }
      cur.push(tok);
      curWidth += tokW;
      continue;
    }
    flushLine();
    if (tokW > maxWidth) {
      const pieces = splitToken(tok, maxWidth, measure);
      for (let k = 0; k < pieces.length - 1; k++) lines.push(pieces[k]);
      cur = pieces[pieces.length - 1];
      curWidth = lineWidth(cur, measure);
    } else {
      cur = [tok];
      curWidth = tokW;
    }
  }
  flushLine();
  return lines;
}

type DrawEl =
  | {
      t: "line";
      tokens: Line;
      x: number;
      y: number;
      lh: number;
      size: number;
      muted: boolean;
    }
  | { t: "codebg"; x: number; y: number; w: number; h: number }
  | { t: "quotebar"; x: number; y: number; h: number }
  | { t: "label"; text: string; x: number; y: number }
  | { t: "hr"; x: number; y: number; w: number };

/** Append "…" to the last visible text line, trimming tokens to fit. */
function ellipsizeLast(
  elements: DrawEl[],
  maxWidth: number,
  measureFor: (size: number) => Measure,
): void {
  for (let i = elements.length - 1; i >= 0; i--) {
    const el = elements[i];
    if (el.t !== "line" || el.tokens.length === 0) continue;
    const measure = measureFor(el.size);
    const avail = maxWidth - el.x;
    const ellipsisW = measure("…", {});
    while (
      el.tokens.length > 0 &&
      lineWidth(el.tokens, measure) + ellipsisW > avail
    ) {
      const last = el.tokens[el.tokens.length - 1];
      if (last.text.length > 1) last.text = last.text.slice(0, -1);
      else el.tokens.pop();
    }
    el.tokens.push({ text: "…", style: {} });
    return;
  }
}

/** Lay out blocks vertically into draw elements within the height budget. */
function layoutBlocks(
  blocks: Block[],
  maxWidth: number,
  measureFor: (size: number) => Measure,
): { elements: DrawEl[]; height: number } {
  const elements: DrawEl[] = [];
  let y = 0;
  let truncated = false;
  let prevKind: Block["kind"] | null = null;

  const addLine = (
    tokens: Line,
    lh: number,
    size: number,
    x = 0,
    muted = false,
  ): boolean => {
    if (y + lh > MAX_BODY_HEIGHT) return false;
    elements.push({ t: "line", tokens, x, y, lh, size, muted });
    y += lh;
    return true;
  };

  for (const block of blocks) {
    if (truncated) break;
    const gap = prevKind === null ? 0 : prevKind === "paragraph" ? 6 : 14;

    switch (block.kind) {
      case "heading": {
        const cfg = HEADING[block.level];
        y += prevKind === null ? 0 : cfg.before;
        // Headings render bold by default.
        const runs = block.runs.map((r) => ({
          ...r,
          style: { ...r.style, bold: true },
        }));
        const lines = wrapTokens(runs, maxWidth, measureFor(cfg.size));
        for (const line of lines) {
          if (!addLine(line, cfg.lh, cfg.size)) {
            truncated = true;
            break;
          }
        }
        break;
      }
      case "paragraph": {
        y += gap;
        const lines = wrapTokens(block.runs, maxWidth, measureFor(BODY_SIZE));
        for (const line of lines) {
          if (!addLine(line, BODY_LH, BODY_SIZE)) {
            truncated = true;
            break;
          }
        }
        break;
      }
      case "quote": {
        y += prevKind === null ? 0 : 14;
        const startIdx = elements.length;
        const startY = y;
        const lines = wrapTokens(
          block.runs,
          maxWidth - QUOTE_INDENT,
          measureFor(BODY_SIZE),
        );
        for (const line of lines) {
          if (!addLine(line, BODY_LH, BODY_SIZE, QUOTE_INDENT, true)) {
            truncated = true;
            break;
          }
        }
        if (y > startY) {
          elements.splice(startIdx, 0, {
            t: "quotebar",
            x: 0,
            y: startY + 4,
            h: y - startY - 8,
          });
        }
        break;
      }
      case "list": {
        y += prevKind === null ? 0 : 14;
        for (const item of block.items) {
          const lines = wrapTokens(
            item.runs,
            maxWidth - LIST_INDENT,
            measureFor(BODY_SIZE),
          );
          if (lines.length === 0) continue;
          if (y + BODY_LH > MAX_BODY_HEIGHT) {
            truncated = true;
            break;
          }
          elements.push({ t: "label", text: item.label, x: 4, y });
          for (const line of lines) {
            if (!addLine(line, BODY_LH, BODY_SIZE, LIST_INDENT)) {
              truncated = true;
              break;
            }
          }
          y += 4; // gap between items
          if (truncated) break;
        }
        break;
      }
      case "table": {
        y += prevKind === null ? 0 : 14;
        const measure = measureFor(TABLE_SIZE);
        const cols = block.header.length;
        if (cols === 0) break;
        const boldHeader = block.header.map((runs) =>
          runs.map((r) => ({ ...r, style: { ...r.style, bold: true } })),
        );

        const cellTextWidth = (runs: InlineRun[]): number => {
          let w = 0;
          for (const tok of tokenize(runs)) w += measure(tok.text, tok.style);
          return w;
        };

        // Natural column widths, then shrink the widest until the table fits.
        const widths = boldHeader.map((runs, k) => {
          let w = cellTextWidth(runs);
          for (const row of block.rows) {
            w = Math.max(w, cellTextWidth(row[k] ?? []));
          }
          return Math.max(w, TABLE_MIN_COL_W);
        });
        const inner = maxWidth - TABLE_COL_GAP * (cols - 1);
        const totalW = () => widths.reduce((a, b) => a + b, 0);
        while (totalW() > inner) {
          let idx = -1;
          for (let k = 0; k < cols; k++) {
            if (
              widths[k] > TABLE_MIN_COL_W &&
              (idx < 0 || widths[k] > widths[idx])
            ) {
              idx = k;
            }
          }
          if (idx < 0) break;
          widths[idx] -= Math.min(
            totalW() - inner,
            widths[idx] - TABLE_MIN_COL_W,
          );
        }
        const colX = widths.map((_, k) => {
          let x = 0;
          for (let j = 0; j < k; j++) x += widths[j] + TABLE_COL_GAP;
          return x;
        });
        const tableW = totalW() + TABLE_COL_GAP * (cols - 1);

        // Emit one visual line per cell-wrapped line; returns false when the
        // height budget runs out mid-row.
        const emitRow = (cells: InlineRun[][]): boolean => {
          const cellLines = cells.map((runs, k) =>
            wrapTokens(runs, widths[k], measure),
          );
          const lineCount = Math.max(1, ...cellLines.map((l) => l.length));
          for (let li = 0; li < lineCount; li++) {
            if (y + TABLE_LH > MAX_BODY_HEIGHT) return false;
            for (let k = 0; k < cols; k++) {
              const line = cellLines[k][li];
              if (!line || line.length === 0) continue;
              const w = lineWidth(line, measure);
              let x = colX[k];
              if (block.align[k] === "center") {
                x += Math.max(0, (widths[k] - w) / 2);
              } else if (block.align[k] === "right") {
                x += Math.max(0, widths[k] - w);
              }
              elements.push({
                t: "line",
                tokens: line,
                x,
                y,
                lh: TABLE_LH,
                size: TABLE_SIZE,
                muted: false,
              });
            }
            y += TABLE_LH;
          }
          return true;
        };

        if (!emitRow(boldHeader)) {
          truncated = true;
          break;
        }
        if (y + 9 > MAX_BODY_HEIGHT) {
          truncated = true;
          break;
        }
        elements.push({ t: "hr", x: 0, y: y + 4, w: tableW });
        y += 9;

        for (let ri = 0; ri < block.rows.length; ri++) {
          if (!emitRow(block.rows[ri])) {
            truncated = true;
            break;
          }
          if (ri < block.rows.length - 1) y += TABLE_ROW_GAP;
        }
        if (!truncated) {
          if (y + 9 > MAX_BODY_HEIGHT) {
            truncated = true;
          } else {
            elements.push({ t: "hr", x: 0, y: y + 4, w: tableW });
            y += 9;
          }
        }
        break;
      }
      case "code": {
        y += prevKind === null ? 0 : 14;
        const measure = measureFor(CODE_SIZE);
        const codeStyle: InlineStyle = { code: true };
        const wrapWidth = maxWidth - CODE_PAD_X * 2;
        const startIdx = elements.length;
        y += CODE_PAD_Y;
        const startY = y;
        for (const src of block.text.split("\n")) {
          if (src === "") {
            if (!addLine([], CODE_LH, CODE_SIZE, CODE_PAD_X)) {
              truncated = true;
              break;
            }
            continue;
          }
          const lines = wrapTokens(
            [{ text: src, style: codeStyle }],
            wrapWidth,
            measure,
            true,
          );
          for (const line of lines) {
            if (!addLine(line, CODE_LH, CODE_SIZE, CODE_PAD_X)) {
              truncated = true;
              break;
            }
          }
          if (truncated) break;
        }
        y += CODE_PAD_Y;
        if (y > startY) {
          elements.splice(startIdx, 0, {
            t: "codebg",
            x: 0,
            y: startY - CODE_PAD_Y,
            w: maxWidth,
            h: y - startY + CODE_PAD_Y,
          });
        }
        break;
      }
      case "hr": {
        y += prevKind === null ? 0 : 12;
        if (y + 13 > MAX_BODY_HEIGHT) {
          truncated = true;
          break;
        }
        elements.push({ t: "hr", x: 0, y: y + 6, w: maxWidth });
        y += 13;
        break;
      }
    }
    prevKind = block.kind;
  }

  if (truncated) ellipsizeLast(elements, maxWidth, measureFor);
  return { elements, height: y };
}

// ── Rendering ──

/** Render the share card to a PNG blob. */
export async function renderShareCard(input: ShareCardInput): Promise<Blob> {
  const colors = themeColors();
  const fontFamily =
    getComputedStyle(document.body).fontFamily || "system-ui, sans-serif";
  const cardWidth = Math.round(
    Math.min(
      MAX_CARD_WIDTH,
      Math.max(MIN_CARD_WIDTH, input.width ?? DEFAULT_CARD_WIDTH),
    ),
  );
  const textWidth = cardWidth - CARD_PAD * 2;

  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas 2D context unavailable");

  const fontFor = (style: InlineStyle, size: number): string => {
    if (style.code) return `${Math.round(size * 0.9)}px ${MONO_FONT}`;
    const italic = style.italic ? "italic " : "";
    const weight = style.bold ? 600 : 400;
    return `${italic}${weight} ${size}px ${fontFamily}`;
  };

  const measureFor = (size: number): Measure => {
    const fontCache = new Map<string, string>();
    return (text, style) => {
      const key = `${style.bold ? 1 : 0}${style.italic ? 1 : 0}${style.code ? 1 : 0}`;
      let font = fontCache.get(key);
      if (!font) {
        font = fontFor(style, size);
        fontCache.set(key, font);
      }
      ctx.font = font;
      return ctx.measureText(text).width;
    };
  };

  // ── Layout pass ──
  const blocks = parseMarkdown(input.content);
  const body =
    blocks.length > 0
      ? layoutBlocks(blocks, textWidth, measureFor)
      : {
          elements: [
            {
              t: "line" as const,
              tokens: [{ text: "(empty)", style: {} }],
              x: 0,
              y: 0,
              lh: BODY_LH,
              size: BODY_SIZE,
              muted: true,
            },
          ],
          height: BODY_LH,
        };

  // All vertical coordinates are canvas-space (origin = canvas top-left).
  const headerY = OUTER_PAD + CARD_PAD;
  const dividerY = headerY + LOGO_SIZE + 20;
  const bodyTop = dividerY + 24;
  const footerTop = bodyTop + body.height + 32;
  const footerHeight = 18;
  const cardHeight = footerTop + footerHeight + CARD_PAD - OUTER_PAD;
  const canvasWidth = cardWidth + OUTER_PAD * 2;
  const canvasHeight = cardHeight + OUTER_PAD * 2;

  // ── Draw pass (resizing clears the canvas) ──
  canvas.width = canvasWidth * SCALE;
  canvas.height = canvasHeight * SCALE;
  ctx.scale(SCALE, SCALE);

  // Outer backdrop
  ctx.fillStyle = colors.background;
  ctx.fillRect(0, 0, canvasWidth, canvasHeight);

  // Card with soft shadow
  ctx.save();
  ctx.shadowColor = "rgba(0, 0, 0, 0.12)";
  ctx.shadowBlur = 24;
  ctx.shadowOffsetY = 6;
  roundedRect(ctx, OUTER_PAD, OUTER_PAD, cardWidth, cardHeight, CARD_RADIUS);
  ctx.fillStyle = colors.card;
  ctx.fill();
  ctx.restore();
  roundedRect(ctx, OUTER_PAD, OUTER_PAD, cardWidth, cardHeight, CARD_RADIUS);
  ctx.strokeStyle = colors.border;
  ctx.lineWidth = 1;
  ctx.stroke();

  const contentX = OUTER_PAD + CARD_PAD;

  // Logo (clipped to rounded square)
  const logo = await loadImage("/yomi.png");
  if (logo) {
    ctx.save();
    roundedRect(ctx, contentX, headerY, LOGO_SIZE, LOGO_SIZE, 10);
    ctx.clip();
    ctx.drawImage(logo, contentX, headerY, LOGO_SIZE, LOGO_SIZE);
    ctx.restore();
  }

  // Wordmark
  ctx.fillStyle = colors.foreground;
  ctx.font = `600 20px ${fontFamily}`;
  ctx.textBaseline = "middle";
  ctx.fillText("Yomi", contentX + LOGO_SIZE + 12, headerY + LOGO_SIZE / 2 + 1);

  // Accent underline beneath header
  const gradient = ctx.createLinearGradient(
    contentX,
    0,
    contentX + textWidth,
    0,
  );
  gradient.addColorStop(0, colors.primary);
  gradient.addColorStop(1, "transparent");
  ctx.fillStyle = gradient;
  ctx.globalAlpha = 0.5;
  roundedRect(ctx, contentX, dividerY, 72, 3, 1.5);
  ctx.fill();
  ctx.globalAlpha = 1;

  // Body elements
  ctx.textBaseline = "top";
  for (const el of body.elements) {
    switch (el.t) {
      case "codebg":
        roundedRect(ctx, contentX + el.x, bodyTop + el.y, el.w, el.h, 8);
        ctx.fillStyle = colors.codeBg;
        ctx.fill();
        break;
      case "quotebar":
        ctx.globalAlpha = 0.5;
        ctx.fillStyle = colors.primary;
        roundedRect(ctx, contentX + el.x, bodyTop + el.y, 3, el.h, 1.5);
        ctx.fill();
        ctx.globalAlpha = 1;
        break;
      case "hr":
        ctx.fillStyle = colors.border;
        ctx.fillRect(contentX + el.x, bodyTop + el.y, el.w, 1);
        break;
      case "label":
        ctx.font = fontFor({}, BODY_SIZE);
        ctx.fillStyle = colors.foreground;
        ctx.fillText(
          el.text,
          contentX + el.x,
          bodyTop + el.y + Math.round((BODY_LH - BODY_SIZE) / 2),
        );
        break;
      case "line": {
        // Pass 1: inline-code backgrounds. They must all paint before any
        // text so a rect never covers the previous token's glyphs.
        let x = contentX + el.x;
        for (const tok of el.tokens) {
          ctx.font = fontFor(tok.style, el.size);
          const w = ctx.measureText(tok.text).width;
          if (tok.style.code) {
            const drawSize = Math.round(el.size * 0.9);
            roundedRect(
              ctx,
              x - 3,
              bodyTop + el.y + (el.lh - drawSize - 8) / 2,
              w + 6,
              drawSize + 8,
              4,
            );
            ctx.fillStyle = colors.codeBg;
            ctx.fill();
          }
          x += w;
        }
        // Pass 2: text and decorations.
        x = contentX + el.x;
        for (const tok of el.tokens) {
          ctx.font = fontFor(tok.style, el.size);
          const drawSize = tok.style.code ? Math.round(el.size * 0.9) : el.size;
          const ty = bodyTop + el.y + Math.round((el.lh - drawSize) / 2);
          const w = ctx.measureText(tok.text).width;
          ctx.fillStyle = tok.style.link
            ? colors.primary
            : el.muted
              ? colors.mutedForeground
              : colors.foreground;
          ctx.fillText(tok.text, x, ty);
          if (tok.style.strike) {
            ctx.fillRect(x, Math.round(ty + drawSize * 0.55), w, 1);
          }
          if (tok.style.link) {
            ctx.globalAlpha = 0.7;
            ctx.fillRect(x, ty + drawSize + 2, w, 1);
            ctx.globalAlpha = 1;
          }
          x += w;
        }
        break;
      }
    }
  }

  // Footer: session title (left) + date (right)
  const footerY = footerTop + footerHeight / 2;
  const footerMeasure = measureFor(13);
  ctx.textBaseline = "middle";
  ctx.font = `13px ${fontFamily}`;
  ctx.fillStyle = colors.mutedForeground;
  if (input.sessionTitle) {
    let title = input.sessionTitle;
    while (title.length > 1 && footerMeasure(title, {}) > textWidth - 180) {
      title = title.slice(0, -1);
    }
    if (title !== input.sessionTitle) title = title.trimEnd() + "…";
    ctx.textAlign = "left";
    ctx.fillText(title, contentX, footerY);
  }
  ctx.textAlign = "right";
  ctx.fillText(formatCardDate(input.date), contentX + textWidth, footerY);
  ctx.textAlign = "left";

  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("Failed to encode PNG"));
    }, "image/png");
  });
}

/**
 * Prompt the user for a save location and write the PNG.
 * Returns the saved path, or null when the user cancelled.
 */
export async function saveShareCard(blob: Blob): Promise<string | null> {
  const pad = (n: number) => String(n).padStart(2, "0");
  const now = new Date();
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const path = await save({
    defaultPath: `yomi-answer-${stamp}.png`,
    filters: [{ name: "PNG Image", extensions: ["png"] }],
  });
  if (!path) return null;
  const buffer = new Uint8Array(await blob.arrayBuffer());
  await writeFile(path, buffer);
  return path;
}

/** Write the PNG to the system clipboard. */
export async function copyShareCardToClipboard(blob: Blob): Promise<void> {
  await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
}
