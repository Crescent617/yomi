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
    dockerfile: "dockerfile",
    makefile: "makefile",
  };
  const basename = (filename.split("/").pop() ?? filename).toLowerCase();
  if (map[basename]) return map[basename];

  const ext = basename.split(".").pop() ?? "";
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

/**
 * Parse a token count with an optional k/m suffix (`512k`, `1m`, `200000`).
 * Returns null for malformed input, zero/negative, and anything above
 * u32::MAX (the IPC payload type) — callers must surface a clear error
 * instead of a bare deserialization failure.
 */
export function parseTokenCount(s: string): number | null {
  const m = s
    .trim()
    .toLowerCase()
    .match(/^(\d+(?:\.\d+)?)([km])?$/);
  if (!m) return null;
  const n = Number.parseFloat(m[1]);
  if (!Number.isFinite(n)) return null;
  const tokens = Math.round(
    m[2] === "k" ? n * 1000 : m[2] === "m" ? n * 1_000_000 : n,
  );
  return tokens > 0 && tokens <= 0xffffffff ? tokens : null;
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
 * rename 输入框的 blur 是不是用户主动离开：Tab/点击造成的焦点转移
 * （relatedTarget 非空）或已武装的外部 pointerdown。反之为 keyed 列表
 * DOM 移动造成的幽灵 blur（Chromium 移焦触发、WebKit 焦点被夺）。
 */
export function isDeliberateRenameExit(
  e: FocusEvent,
  pointerDismiss: boolean,
): boolean {
  return e.relatedTarget !== null || pointerDismiss;
}

/**
 * 私用区字符（U+E000–U+F8FF）匹配。macOS 输入法怪癖：方向键/功能键
 * 的键码（U+F700–U+F704）可能经 `insertText` 漏进 textarea，显示为
 * 豆腐块。私用区没有任何合法输入内容。
 */
const PUA_RE = /[\u{e000}-\u{f8ff}]/gu;

export function containsPua(text: string): boolean {
  PUA_RE.lastIndex = 0;
  return PUA_RE.test(text);
}

export function stripPua(text: string): string {
  return text.replace(PUA_RE, "");
}

/**
 * 拦截输入框里的私用区字符。两条注入路径：beforeinput 能看到的所有
 * insert*（逐键 insertText、组字提交 insertCompositionText、系统文本
 * 替换 insertReplacementText、拖拽/Yank 等——粘贴 insertFromPaste 除
 * 外，整体拒掉会误伤长文本，由 `sanitizePuaPaste` 消毒后插入）。
 */
export function blockPuaInput(e: InputEvent) {
  if (e.inputType === "insertFromPaste") return;
  if (!e.inputType.startsWith("insert")) return;
  if (e.data && containsPua(e.data)) e.preventDefault();
}

/**
 * input 事件兜底：少数泄漏路径（输入法/系统命令直接写值）可能不发
 * beforeinput，或事件形态不在拦截表内。捕获阶段把值里的 PUA 剥掉、
 * 光标按剥除数折算回原位——在 Svelte bind:value 读取之前完成，组件
 * 拿到的已是干净值。组字中的 marked text 不动（提交时由上面的
 * beforeinput 拦截）。
 */
export function stripPuaOnInput(e: Event) {
  if ((e as InputEvent).isComposing) return;
  // 鸭子判断而非 instanceof：单测跑在 node 环境（无 DOM 类）。
  const el = e.target as HTMLInputElement | HTMLTextAreaElement | null;
  if (
    !el ||
    typeof el.value !== "string" ||
    typeof el.setSelectionRange !== "function"
  ) {
    return;
  }
  const value = el.value;
  if (!containsPua(value)) return;
  // email/number 等类型读 selectionStart 会抛 InvalidStateError —— 读到
  // 就双端各自折算（保留选区），读不到退化为末尾光标。
  let start: number;
  let end: number;
  try {
    start = el.selectionStart ?? value.length;
    end = el.selectionEnd ?? start;
  } catch {
    start = end = value.length;
  }
  const rebase = (pos: number) => {
    const before = value.slice(0, pos);
    return pos - (before.length - stripPua(before).length);
  };
  el.value = stripPua(value);
  try {
    el.setSelectionRange(rebase(start), rebase(end));
  } catch {
    // number 等不支持 selection 的输入类型——剥字符已达成，光标随引擎。
  }
}

/**
 * 全局私用区守卫：capture 阶段的 beforeinput 拦截 + input 兜底剥除，
 * 一次覆盖当前与未来的全部文本框（聊天输入、搜索框、重命名、面板
 * 表单……），免去逐组件接线。粘贴不进全局层（需要按字段消毒后手动
 * 插入）：ChatInput/AskUserBar 有各自的 onpaste 消毒，其余字段的粘
 * 贴靠 input 兜底剥除（撤销栈会被 value setter 清空，可接受）。
 */
export function installGlobalPuaGuard(): () => void {
  const onBeforeInput = (e: Event) => blockPuaInput(e as InputEvent);
  window.addEventListener("beforeinput", onBeforeInput, true);
  window.addEventListener("input", stripPuaOnInput, true);
  return () => {
    window.removeEventListener("beforeinput", onBeforeInput, true);
    window.removeEventListener("input", stripPuaOnInput, true);
  };
}

/**
 * 粘贴消毒：剪贴板文本含私用区字符时，拦掉默认粘贴、把剥掉 PUA 的
 * 文本手动插到光标处（dispatch input 事件让 Svelte 绑定同步；浏览器
 * 撤销栈对 setRangeText 的支持各引擎不一，不作为语义保证）。无 PUA
 * 时不动，走浏览器默认粘贴。
 */
export function sanitizePuaPaste(
  e: ClipboardEvent,
  textarea: HTMLTextAreaElement,
) {
  const text = e.clipboardData?.getData("text/plain");
  if (!text || !containsPua(text)) return;
  e.preventDefault();
  textarea.setRangeText(
    stripPua(text),
    textarea.selectionStart,
    textarea.selectionEnd,
    "end",
  );
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}
