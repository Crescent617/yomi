import type { BundledLanguage, Highlighter } from "shiki";

const MAX_CACHE_ENTRIES = 128;
const MAX_HIGHLIGHT_CHARS = 100_000;
const MAX_HIGHLIGHT_LINES = 3_000;

const languageAliases: Record<string, string> = {
  cjs: "javascript",
  html: "html",
  js: "javascript",
  jsx: "jsx",
  md: "markdown",
  mjs: "javascript",
  plaintext: "text",
  py: "python",
  rb: "ruby",
  rs: "rust",
  sh: "bash",
  shell: "bash",
  ts: "typescript",
  tsx: "tsx",
  txt: "text",
  yml: "yaml",
  zsh: "bash",
};

let highlighterPromise: Promise<Highlighter> | undefined;
const languageLoads = new Map<string, Promise<void>>();
const highlightCache = new Map<string, Promise<string | null>>();

export function normalizeCodeLanguage(language: string | undefined): string {
  const normalized = language?.trim().toLowerCase() || "text";
  return languageAliases[normalized] ?? normalized;
}

export function shouldHighlightCode(code: string, language: string): boolean {
  if (language === "text" || language === "mermaid") return false;
  if (code.length > MAX_HIGHLIGHT_CHARS) return false;
  return (
    code.split("\n", MAX_HIGHLIGHT_LINES + 1).length <= MAX_HIGHLIGHT_LINES
  );
}

async function getHighlighter(): Promise<Highlighter> {
  highlighterPromise ??= import("shiki").then(({ createHighlighter }) =>
    createHighlighter({
      themes: ["github-light", "github-dark"],
      langs: [],
    }),
  );
  return highlighterPromise;
}

async function ensureLanguage(
  highlighter: Highlighter,
  language: string,
): Promise<void> {
  if (highlighter.getLoadedLanguages().includes(language)) return;

  let loading = languageLoads.get(language);
  if (!loading) {
    loading = highlighter
      .loadLanguage(language as BundledLanguage)
      .then(() => undefined);
    languageLoads.set(language, loading);
  }

  try {
    await loading;
  } catch (error) {
    languageLoads.delete(language);
    throw error;
  }
}

async function renderHighlightedCode(
  code: string,
  language: string,
): Promise<string | null> {
  if (!shouldHighlightCode(code, language)) return null;

  try {
    const highlighter = await getHighlighter();
    await ensureLanguage(highlighter, language);
    return highlighter.codeToHtml(code, {
      lang: language,
      themes: {
        light: "github-light",
        dark: "github-dark",
      },
      defaultColor: "light",
    });
  } catch {
    // Unknown languages and unavailable grammars remain readable as plain text.
    return null;
  }
}

function setCachedHighlight(key: string, value: Promise<string | null>) {
  if (highlightCache.size >= MAX_CACHE_ENTRIES) {
    const oldestKey = highlightCache.keys().next().value;
    if (oldestKey !== undefined) highlightCache.delete(oldestKey);
  }
  highlightCache.set(key, value);
}

/** Highlight once for both themes. Theme changes are handled entirely by CSS. */
export function highlightCode(
  code: string,
  language: string | undefined,
): Promise<string | null> {
  const normalizedLanguage = normalizeCodeLanguage(language);
  const key = `${normalizedLanguage}\0${code}`;
  const cached = highlightCache.get(key);
  if (cached) {
    highlightCache.delete(key);
    highlightCache.set(key, cached);
    return cached;
  }

  const task = renderHighlightedCode(code, normalizedLanguage);
  setCachedHighlight(key, task);
  return task;
}
