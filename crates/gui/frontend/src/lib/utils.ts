import { invoke } from "@tauri-apps/api/core";

export function formatShortId(id: string): string {
  return id.slice(-8);
}

/** Deterministic, theme-aware accent color for a project (hash of name+dir → hue). */
export function projectColor(seed: string): string {
  let h = 0;
  for (let i = 0; i < seed.length; i++) {
    h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  }
  const hue = h % 360;
  return `color-mix(in oklab, hsl(${hue} 55% 55%) 45%, hsl(var(--muted-foreground)))`;
}

export function formatTimeAgo(
  date: Date | string,
  nowMs: number = Date.now(),
): string {
  const then = typeof date === "string" ? new Date(date) : date;
  const diff = Math.floor((nowMs - then.getTime()) / 1000);

  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  return then.toLocaleDateString();
}

export function detectLang(filename: string): string {
  const map: Record<string, string> = {
    Dockerfile: "dockerfile",
    Makefile: "makefile",
  };
  const basename = filename.split("/").pop() ?? filename;
  if (map[basename]) return map[basename];

  const ext = basename.split(".").pop()?.toLowerCase() ?? "";
  const extensionMap: Record<string, string> = {
    rs: "rust",
    js: "javascript",
    ts: "typescript",
    jsx: "javascript",
    tsx: "typescript",
    py: "python",
    go: "go",
    java: "java",
    c: "c",
    cpp: "cpp",
    h: "c",
    hpp: "cpp",
    md: "markdown",
    json: "json",
    yaml: "yaml",
    yml: "yaml",
    toml: "toml",
    html: "html",
    css: "css",
    scss: "scss",
    sql: "sql",
    sh: "bash",
    bash: "bash",
    zsh: "bash",
  };
  return extensionMap[ext] ?? "plaintext";
}

/**
 * Extensions that open in the in-app text preview (Markdown rendered,
 * everything else syntax-highlighted) instead of the system default app.
 * Binary-looking types stay external-only.
 */
const TEXT_PREVIEW_EXTENSIONS = new Set([
  "md",
  "markdown",
  "txt",
  "log",
  "json",
  "jsonl",
  "csv",
  "tsv",
  "xml",
  "yaml",
  "yml",
  "toml",
  "ini",
  "env",
  "rs",
  "js",
  "mjs",
  "cjs",
  "ts",
  "jsx",
  "tsx",
  "py",
  "go",
  "java",
  "c",
  "h",
  "cpp",
  "hpp",
  "cs",
  "rb",
  "php",
  "swift",
  "kt",
  "sql",
  "sh",
  "bash",
  "zsh",
  "html",
  "css",
  "scss",
  "svg",
  "diff",
  "patch",
]);

/** Well-known extensionless text files (matched case-insensitively). */
const TEXT_PREVIEW_BASENAMES = new Set([
  "dockerfile",
  "makefile",
  "license",
  "notice",
]);

/** Whether an attachment path opens in the in-app text preview on click. */
export function isTextPreviewable(path: string): boolean {
  const basename = (path.split(/[/\\]/).pop() ?? path).toLowerCase();
  if (TEXT_PREVIEW_BASENAMES.has(basename)) return true;
  if (!basename.includes(".")) return false;
  const ext = basename.split(".").pop() ?? "";
  return TEXT_PREVIEW_EXTENSIONS.has(ext);
}

export function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function tokenEstimate(text: string): string {
  const n = Math.round(utf8ByteLength(text) / 4);
  if (n >= 1000) return `~${(n / 1000).toFixed(1)}k`;
  return `~${n}`;
}

export function formatTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return `${n}`;
}

// Count UTF-8 bytes to match Rust's text.len() for token estimation.
export function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

export function formatMessageTime(iso: string | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "";
  const now = new Date();
  const isToday = d.toDateString() === now.toDateString();
  const timeStr = d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  if (isToday) return timeStr;
  const dateStr = d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
  return `${dateStr} ${timeStr}`;
}

/**
 * Collapse the user's home directory prefix to `~`.
 * Compatible with both Unix (`/home/user/...`) and Windows (`C:\Users\user\...`).
 *
 * `home` must be provided (obtain via `@tauri-apps/api/path` `homeDir()` in Tauri).
 * If `home` is empty, returns `path` unchanged.
 */
export function collapseHome(path: string, home: string): string {
  if (!home) return path;
  const normalizedHome = home.replace(/\\/g, "/").replace(/\/$/, "");
  const normalizedPath = path.replace(/\\/g, "/");
  if (
    normalizedPath === normalizedHome ||
    normalizedPath.startsWith(normalizedHome + "/")
  ) {
    return "~" + normalizedPath.slice(normalizedHome.length);
  }
  return path;
}

// LRU cache for asset blob URLs (max 64 entries) to prevent memory leaks.
const MAX_ASSET_CACHE = 64;
const assetCache = new Map<string, string>();

function setAssetCache(url: string, blobUrl: string) {
  if (assetCache.size >= MAX_ASSET_CACHE && !assetCache.has(url)) {
    const firstKey = assetCache.keys().next().value;
    if (firstKey) {
      const old = assetCache.get(firstKey);
      if (old) URL.revokeObjectURL(old);
      assetCache.delete(firstKey);
    }
  }
  assetCache.set(url, blobUrl);
}

export async function resolveAssetUrl(url: string): Promise<string> {
  if (!url.startsWith("asset://")) return url;
  const cached = assetCache.get(url);
  if (cached) return cached;
  try {
    const bytes: number[] = await invoke("read_asset", { url });
    const blob = new Blob([new Uint8Array(bytes)]);
    const blobUrl = URL.createObjectURL(blob);
    setAssetCache(url, blobUrl);
    return blobUrl;
  } catch (e) {
    console.error("Failed to resolve asset:", e);
    return url;
  }
}

/** Svelte action: focus the input and select its contents on mount. */
export function focusAndSelect(node: HTMLInputElement) {
  node.focus();
  node.select();
}

/**
 * 拦截输入框里的私用区字符（U+E000–U+F8FF）。
 * macOS 输入法怪癖：方向键/功能键的键码（U+F700–U+F704）可能经由
 * `insertText` 漏进 textarea，显示为豆腐块。私用区没有任何合法输入内容，
 * 直接拒掉。（输入法正常组字走 insertCompositionText，不受影响。）
 */
export function blockPuaInput(e: InputEvent) {
  if (e.inputType !== "insertText" || !e.data) return;
  for (const ch of e.data) {
    const cp = ch.codePointAt(0)!;
    if (cp >= 0xe000 && cp <= 0xf8ff) {
      e.preventDefault();
      return;
    }
  }
}
