import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

const mermaidMocks = vi.hoisted(() => ({
  initialize: vi.fn(),
  render: vi.fn<
    (
      id: string,
      source: string,
    ) => Promise<{
      svg: string;
      bindFunctions?: (element: Element) => void;
    }>
  >(async (_id, source) => ({ svg: `<svg>${source}</svg>` })),
}));

vi.mock("mermaid", () => ({ default: mermaidMocks }));
vi.mock("./mermaidTheme", () => ({ mermaidTheme: () => ({}) }));

type IdleCallback = (deadline: IdleDeadline) => void;

type BrowserMocks = {
  idleCallbacks: Map<number, IdleCallback>;
  window: EventTarget & {
    requestIdleCallback: typeof requestIdleCallback;
    cancelIdleCallback: typeof cancelIdleCallback;
  };
};

function installBrowserMocks(): BrowserMocks {
  let nextIdleId = 0;
  const idleCallbacks = new Map<number, IdleCallback>();
  const windowMock = new EventTarget() as BrowserMocks["window"];
  windowMock.requestIdleCallback = vi.fn((callback: IdleCallback) => {
    const id = ++nextIdleId;
    idleCallbacks.set(id, callback);
    return id;
  });
  windowMock.cancelIdleCallback = vi.fn((id: number) => {
    idleCallbacks.delete(id);
  });

  vi.stubGlobal("window", windowMock);
  vi.stubGlobal("document", {
    documentElement: { classList: { contains: () => false } },
  });
  vi.stubGlobal(
    "DOMParser",
    class {
      parseFromString(svg: string) {
        return {
          documentElement: { svg },
          querySelectorAll: () => [],
        };
      }
    },
  );
  vi.stubGlobal(
    "XMLSerializer",
    class {
      serializeToString(element: { svg: string }) {
        return element.svg;
      }
    },
  );
  vi.spyOn(performance, "now").mockReturnValue(1_000);
  return { idleCallbacks, window: windowMock };
}

async function waitForIdleCount(
  idleCallbacks: Map<number, IdleCallback>,
  count: number,
) {
  for (let i = 0; i < 20 && idleCallbacks.size < count; i++) {
    await Promise.resolve();
  }
  expect(idleCallbacks.size).toBe(count);
}

function runOnlyIdleCallback(idleCallbacks: Map<number, IdleCallback>) {
  const entry = idleCallbacks.entries().next().value as
    | [number, IdleCallback]
    | undefined;
  expect(entry).toBeDefined();
  if (!entry) return;
  idleCallbacks.delete(entry[0]);
  entry[1]({ didTimeout: false, timeRemaining: () => 50 });
}

describe("Mermaid render queue", () => {
  beforeEach(() => {
    vi.resetModules();
    mermaidMocks.initialize.mockClear();
    mermaidMocks.render.mockClear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  test("cancelled idle work does not block a new render with the same key", async () => {
    const { idleCallbacks, window } = installBrowserMocks();
    const { renderMermaid } = await import("./mermaid");
    const controller = new AbortController();
    const source = "graph TD; A-->B";

    const cancelled = renderMermaid(source, controller.signal);
    await waitForIdleCount(idleCallbacks, 1);
    controller.abort();

    await expect(cancelled).rejects.toMatchObject({ name: "AbortError" });
    expect(window.cancelIdleCallback).toHaveBeenCalledTimes(1);
    expect(idleCallbacks.size).toBe(0);

    const live = renderMermaid(source);
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);

    await expect(live).resolves.toMatchObject({ svg: `<svg>${source}</svg>` });
    expect(mermaidMocks.render).toHaveBeenCalledTimes(1);
  });

  test("keeps shared work alive while another consumer remains", async () => {
    const { idleCallbacks, window } = installBrowserMocks();
    const { renderMermaid } = await import("./mermaid");
    const firstController = new AbortController();
    const source = "graph TD; Shared-->Task";

    const first = renderMermaid(source, firstController.signal);
    const second = renderMermaid(source);
    await waitForIdleCount(idleCallbacks, 1);
    firstController.abort();

    await expect(first).rejects.toMatchObject({ name: "AbortError" });
    expect(window.cancelIdleCallback).not.toHaveBeenCalled();
    runOnlyIdleCallback(idleCallbacks);

    await expect(second).resolves.toMatchObject({
      svg: `<svg>${source}</svg>`,
    });
    expect(mermaidMocks.render).toHaveBeenCalledTimes(1);
  });

  test("theme events invalidate cached results even when resolved theme is unchanged", async () => {
    const { idleCallbacks, window } = installBrowserMocks();
    const { renderMermaid } = await import("./mermaid");
    const source = "graph TD; Theme-->Refresh";

    const first = renderMermaid(source);
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);
    await first;

    window.dispatchEvent(new Event("theme-changed"));
    const second = renderMermaid(source);
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);
    await second;

    expect(mermaidMocks.render).toHaveBeenCalledTimes(2);
  });

  test("cancelled in-progress work is not cached", async () => {
    const { idleCallbacks } = installBrowserMocks();
    let finishRender: ((value: { svg: string }) => void) | undefined;
    mermaidMocks.render.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRender = resolve;
        }),
    );
    const { renderMermaid } = await import("./mermaid");
    const controller = new AbortController();
    const source = "graph TD; Cancelled-->Render";

    const cancelled = renderMermaid(source, controller.signal);
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);
    for (let i = 0; i < 20 && !finishRender; i++) await Promise.resolve();
    expect(finishRender).toBeDefined();

    controller.abort();
    await expect(cancelled).rejects.toMatchObject({ name: "AbortError" });
    finishRender?.({ svg: `<svg>${source}</svg>` });
    await Promise.resolve();

    const live = renderMermaid(source);
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);
    await live;

    expect(mermaidMocks.render).toHaveBeenCalledTimes(2);
  });

  test("cache hits preserve Mermaid bind functions", async () => {
    const { idleCallbacks } = installBrowserMocks();
    const bindFunctions = vi.fn();
    mermaidMocks.render.mockResolvedValueOnce({
      svg: "<svg>bound</svg>",
      bindFunctions,
    });
    const { renderMermaid } = await import("./mermaid");

    const first = renderMermaid("graph TD; Bound-->Diagram");
    await waitForIdleCount(idleCallbacks, 1);
    runOnlyIdleCallback(idleCallbacks);
    await first;
    const cached = await renderMermaid("graph TD; Bound-->Diagram");

    expect(cached.bindFunctions).toBe(bindFunctions);
    expect(mermaidMocks.render).toHaveBeenCalledTimes(1);
  });
});
