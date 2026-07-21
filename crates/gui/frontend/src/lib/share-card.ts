/**
 * Share card renderer: draws an assistant answer as a branded PNG card
 * using an offscreen canvas. Theme colors are read from the app's CSS
 * variables so the card matches light/dark mode.
 */

import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { markdownToPlainText, wrapText } from "./share-text";

const SCALE = 2;
const CARD_WIDTH = 720;
const CARD_PAD = 48;
const OUTER_PAD = 32;
const CARD_RADIUS = 20;
const LOGO_SIZE = 40;
const BODY_FONT_SIZE = 17;
const BODY_LINE_HEIGHT = 30;
const MAX_BODY_LINES = 24;

export interface ShareCardInput {
  /** Markdown source of the answer */
  content: string;
  sessionTitle?: string;
  date: Date;
}

interface ThemeColors {
  background: string;
  card: string;
  foreground: string;
  mutedForeground: string;
  border: string;
  primary: string;
}

/** Read a shadcn-style HSL channel variable (e.g. "210 40% 98%") as a CSS color. */
function cssHslVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return value ? `hsl(${value})` : fallback;
}

function themeColors(): ThemeColors {
  return {
    background: cssHslVar("--background", "#ffffff"),
    card: cssHslVar("--card", "#f8f9fa"),
    foreground: cssHslVar("--foreground", "#1a1d21"),
    mutedForeground: cssHslVar("--muted-foreground", "#6b7280"),
    border: cssHslVar("--border", "#e5e7eb"),
    primary: cssHslVar("--primary", "#3b82f6"),
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

/** Render the share card to a PNG blob. */
export async function renderShareCard(input: ShareCardInput): Promise<Blob> {
  const colors = themeColors();
  const fontFamily =
    getComputedStyle(document.body).fontFamily || "system-ui, sans-serif";
  const bodyFont = `${BODY_FONT_SIZE}px ${fontFamily}`;
  const textWidth = CARD_WIDTH - CARD_PAD * 2;

  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas 2D context unavailable");

  // ── Measure pass: wrap body text to compute card height ──
  // (no need to scale here: measureText ignores the transform)
  ctx.font = bodyFont;
  const measure = (t: string) => ctx.measureText(t).width;
  const plain = markdownToPlainText(input.content);
  let lines = wrapText(plain || "(empty)", textWidth, measure);
  if (lines.length > MAX_BODY_LINES) {
    lines = lines.slice(0, MAX_BODY_LINES);
    // Ellipsize at a word boundary when possible; keep single-token lines.
    const last = lines[MAX_BODY_LINES - 1];
    const trimmed = last.replace(/\s*\S*$/, "");
    lines[MAX_BODY_LINES - 1] = `${trimmed || last} …`;
  }

  // All vertical coordinates are canvas-space (origin = canvas top-left).
  const headerY = OUTER_PAD + CARD_PAD;
  const dividerY = headerY + LOGO_SIZE + 20;
  const bodyTop = dividerY + 24;
  const bodyHeight = lines.length * BODY_LINE_HEIGHT;
  const footerTop = bodyTop + bodyHeight + 32;
  const footerHeight = 18;
  const cardHeight = footerTop + footerHeight + CARD_PAD - OUTER_PAD;
  const canvasWidth = CARD_WIDTH + OUTER_PAD * 2;
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
  roundedRect(ctx, OUTER_PAD, OUTER_PAD, CARD_WIDTH, cardHeight, CARD_RADIUS);
  ctx.fillStyle = colors.card;
  ctx.fill();
  ctx.restore();
  roundedRect(ctx, OUTER_PAD, OUTER_PAD, CARD_WIDTH, cardHeight, CARD_RADIUS);
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

  // Body text
  ctx.font = bodyFont;
  ctx.fillStyle = colors.foreground;
  ctx.textBaseline = "top";
  lines.forEach((line, i) => {
    ctx.fillText(line, contentX, bodyTop + i * BODY_LINE_HEIGHT);
  });

  // Footer: session title (left) + date (right)
  const footerY = footerTop + footerHeight / 2;
  ctx.textBaseline = "middle";
  ctx.font = `13px ${fontFamily}`;
  ctx.fillStyle = colors.mutedForeground;
  if (input.sessionTitle) {
    let title = input.sessionTitle;
    while (title.length > 1 && measure(title) > textWidth - 180) {
      title = title.slice(0, -1);
    }
    if (title !== input.sessionTitle) title = title.trimEnd() + "…";
    ctx.textAlign = "left";
    ctx.fillText(title, contentX, footerY);
  }
  ctx.textAlign = "right";
  ctx.fillText(
    formatCardDate(input.date),
    contentX + textWidth,
    footerY,
  );
  ctx.textAlign = "left";

  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("Failed to encode PNG"));
    }, "image/png");
  });
}

/**
 * Render the card and prompt the user for a save location.
 * Returns the saved path, or null when the user cancelled.
 */
export async function shareAnswerAsImage(
  input: ShareCardInput,
): Promise<string | null> {
  const blob = await renderShareCard(input);
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
