import { mermaidTheme } from "./mermaidTheme";

const MAX_SOURCE_LENGTH = 50_000;
const MAX_CACHE_ENTRIES = 32;
const MAX_CACHE_BYTES = 4 * 1024 * 1024;
const SCROLL_IDLE_MS = 300;

let renderId = 0;
let renderQueue: Promise<unknown> = Promise.resolve();
let themeEpoch = 0;
let cacheBytes = 0;
let lastScrollAt = 0;

export interface MermaidRenderResult {
  svg: string;
  bindFunctions?: (element: Element) => void;
}

type Theme = "light" | "dark";

type RenderTask = {
  controller: AbortController;
  consumers: Set<symbol>;
  epoch: number;
  promise: Promise<MermaidRenderResult>;
};

type CacheEntry = {
  svg: string;
  bindFunctions?: (element: Element) => void;
  bytes: number;
};

const inFlight = new Map<string, RenderTask>();
const cache = new Map<string, CacheEntry>();

if (typeof window !== "undefined") {
  window.addEventListener("scroll", () => (lastScrollAt = performance.now()), {
    capture: true,
    passive: true,
  });
  window.addEventListener("theme-changed", () => {
    themeEpoch += 1;
  });
}

/**
 * Render one diagram at a time because Mermaid configuration is global.
 * Same-theme, same-source work is shared, while each caller can stop waiting
 * independently through its own AbortSignal.
 */
export function renderMermaid(
  source: string,
  signal?: AbortSignal,
): Promise<MermaidRenderResult> {
  if (signal?.aborted) return Promise.reject(abortError());
  try {
    validateSource(source);
  } catch (error) {
    return Promise.reject(error);
  }

  const theme = resolvedTheme();
  const key = `${themeEpoch}\0${theme}\0${source}`;
  const cached = takeCached(key);
  if (cached) return Promise.resolve(cached);

  let task = inFlight.get(key);
  if (!task || task.epoch !== themeEpoch || task.controller.signal.aborted) {
    task = createRenderTask(key, source, themeEpoch);
    inFlight.set(key, task);
  }

  return consume(task, signal);
}

function createRenderTask(
  key: string,
  source: string,
  epoch: number,
): RenderTask {
  const task: RenderTask = {
    controller: new AbortController(),
    consumers: new Set<symbol>(),
    epoch,
    promise: Promise.resolve(undefined as never),
  };

  task.promise = renderQueue.then(async () => {
    if (task.consumers.size === 0) throw abortError();
    if (task.epoch !== themeEpoch) throw abortError("The theme changed.");
    await waitForIdle(task.controller.signal);
    if (task.consumers.size === 0) throw abortError();
    if (task.epoch !== themeEpoch) throw abortError("The theme changed.");

    const result = await renderMermaidInternal(source);
    if (task.controller.signal.aborted || task.consumers.size === 0) {
      throw abortError();
    }
    if (task.epoch !== themeEpoch) throw abortError("The theme changed.");
    putCached(key, result);
    return result;
  });
  renderQueue = task.promise.catch(() => undefined);
  void task.promise.then(
    () => {
      if (inFlight.get(key) === task) inFlight.delete(key);
    },
    () => {
      if (inFlight.get(key) === task) inFlight.delete(key);
    },
  );
  return task;
}

function consume(
  task: RenderTask,
  signal?: AbortSignal,
): Promise<MermaidRenderResult> {
  const consumer = Symbol();
  task.consumers.add(consumer);

  return new Promise((resolve, reject) => {
    const stop = () => {
      const removed = task.consumers.delete(consumer);
      signal?.removeEventListener("abort", onAbort);
      if (removed && task.consumers.size === 0) task.controller.abort();
    };
    const onAbort = () => {
      stop();
      reject(abortError());
    };

    signal?.addEventListener("abort", onAbort, { once: true });
    task.promise.then(
      (result) => {
        stop();
        resolve(result);
      },
      (error) => {
        stop();
        reject(error);
      },
    );
  });
}

function waitForIdle(signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.reject(abortError());

  return new Promise((resolve, reject) => {
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    let idleId: number | undefined;
    let frameId: number | undefined;
    let settled = false;

    const cleanup = () => {
      if (timeoutId !== undefined) clearTimeout(timeoutId);
      if (
        idleId !== undefined &&
        typeof window.cancelIdleCallback === "function"
      ) {
        window.cancelIdleCallback(idleId);
      }
      if (frameId !== undefined) cancelAnimationFrame(frameId);
      signal.removeEventListener("abort", onAbort);
    };
    const succeed = () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve();
    };
    const onAbort = () => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(abortError());
    };
    const finish = () => {
      if (performance.now() - lastScrollAt < SCROLL_IDLE_MS) schedule();
      else succeed();
    };
    const schedule = () => {
      if (settled) return;
      const remaining = SCROLL_IDLE_MS - (performance.now() - lastScrollAt);
      if (remaining > 0) {
        timeoutId = setTimeout(() => {
          timeoutId = undefined;
          schedule();
        }, remaining);
        return;
      }
      if (typeof window.requestIdleCallback === "function") {
        idleId = window.requestIdleCallback(
          () => {
            idleId = undefined;
            finish();
          },
          { timeout: 500 },
        );
      } else {
        frameId = requestAnimationFrame(() => {
          frameId = undefined;
          timeoutId = setTimeout(() => {
            timeoutId = undefined;
            finish();
          }, 0);
        });
      }
    };

    signal.addEventListener("abort", onAbort, { once: true });
    schedule();
  });
}

function resolvedTheme(): Theme {
  return document.documentElement.classList.contains("dark") ? "dark" : "light";
}

function takeCached(key: string): MermaidRenderResult | undefined {
  const entry = cache.get(key);
  if (!entry) return undefined;
  cache.delete(key);
  cache.set(key, entry);
  return { svg: entry.svg, bindFunctions: entry.bindFunctions };
}

function putCached(key: string, result: MermaidRenderResult) {
  const bytes = new TextEncoder().encode(result.svg).byteLength;
  if (bytes > MAX_CACHE_BYTES) return;

  const existing = cache.get(key);
  if (existing) {
    cacheBytes -= existing.bytes;
    cache.delete(key);
  }
  cache.set(key, {
    svg: result.svg,
    bindFunctions: result.bindFunctions,
    bytes,
  });
  cacheBytes += bytes;

  while (cache.size > MAX_CACHE_ENTRIES || cacheBytes > MAX_CACHE_BYTES) {
    const oldestKey = cache.keys().next().value;
    if (oldestKey === undefined) break;
    const oldest = cache.get(oldestKey);
    cache.delete(oldestKey);
    cacheBytes -= oldest?.bytes ?? 0;
  }
}

function validateSource(source: string) {
  if (!source.trim()) throw new Error("The diagram is empty.");
  if (source.length > MAX_SOURCE_LENGTH) {
    throw new Error("The diagram is too large to render.");
  }
}

function abortError(message = "The render was aborted."): DOMException {
  return new DOMException(message, "AbortError");
}

async function renderMermaidInternal(
  source: string,
): Promise<MermaidRenderResult> {
  const { default: mermaid } = await import("mermaid");
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    theme: "base",
    htmlLabels: false,
    maxTextSize: MAX_SOURCE_LENGTH,
    themeVariables: mermaidTheme(),
  });

  renderId += 1;
  const result = await mermaid.render(`yomi-mermaid-${renderId}`, source);
  return {
    svg: sanitizeSvg(result.svg),
    bindFunctions: result.bindFunctions,
  };
}

/** Defense in depth on top of Mermaid's strict security mode. */
function sanitizeSvg(svg: string): string {
  const document = new DOMParser().parseFromString(svg, "image/svg+xml");
  document
    .querySelectorAll("script, foreignObject")
    .forEach((node) => node.remove());
  for (const element of document.querySelectorAll("*")) {
    for (const attribute of [...element.attributes]) {
      if (
        attribute.name.toLowerCase().startsWith("on") ||
        /^(?:javascript|data):/i.test(attribute.value.trim())
      ) {
        element.removeAttribute(attribute.name);
      }
    }
  }
  return new XMLSerializer().serializeToString(document.documentElement);
}
