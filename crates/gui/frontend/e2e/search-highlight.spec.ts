import { expect, test } from "@playwright/test";

// Regression spec for the 2026-09-05 search-highlight bug report:
//  a) partial/stale highlight when a match can't be located in the DOM
//  b) stale highlight surviving a query edit
//  c) highlight surviving search close
// Root cause class: the painted range was only ever replaced on success,
// so every failure path (element missing, occurrence not locatable in the
// rendered DOM, stale tick callback) left the previous range painted.

async function mountList(page: import("@playwright/test").Page) {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 640 });
  await page.emulateMedia({ reducedMotion: "reduce" });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const created_at = new Date().toISOString();
    const messages = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text" as const, text: "g" }],
        created_at,
      },
      {
        id: "a1",
        type: "assistant" as const,
        // The marker is stripped for display, so END_TURN matches the data
        // layer but is un-locatable in the rendered DOM.
        content: [{ type: "text" as const, text: "done __YOMI_END_TURN__" }],
        created_at,
      },
    ];
    const session = sessionLib.createSessionState({
      id: "search-stale-test",
      messages,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="search-stale-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>("#search-stale-test");
    if (!target) throw new Error("missing mount target");
    mount(MessageList, { target });
    await tick();
  });
}

function highlightTexts(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const registry = (CSS as any).highlights;
    const h = registry?.get("yomi-search-active");
    if (!h) return [] as string[];
    const out: string[] = [];
    for (const r of h as Iterable<Range>) out.push(r.toString());
    return out;
  });
}

test("un-locatable match never leaves stale paint; close clears", async ({
  page,
}) => {
  await mountList(page);

  await page.keyboard.press("ControlOrMeta+f");
  const searchbar = page.getByRole("search", { name: "Search messages" });
  const input = searchbar.getByRole("textbox", { name: "Search messages" });
  await expect(input).toBeFocused();

  // "g" paints; growing the query keeps tracking the locatable match.
  await input.pressSequentially("g");
  await expect.poll(() => highlightTexts(page)).toEqual(["g"]);

  // Switch to a query whose only match is un-locatable (display-stripped
  // marker): the previous range must be CLEARED, not left behind.
  await input.fill("END_TURN");
  await expect(searchbar.getByText(/^1\/1$/)).toBeVisible();
  await expect.poll(() => highlightTexts(page)).toEqual([]);

  // Back to a locatable query, then close: nothing may stay painted.
  await input.fill("g");
  await expect.poll(() => highlightTexts(page)).toEqual(["g"]);
  await input.press("Escape");
  await expect(searchbar).toHaveCount(0);
  await expect.poll(() => highlightTexts(page)).toEqual([]);
});

test("streaming re-render re-pins the highlight to the live DOM", async ({
  page,
}) => {
  await mountList(page);

  // Attach a streaming message; search lands inside it.
  await page.evaluate(async () => {
    const { tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    state.streamingMessages["search-stale-test"] = [
      {
        id: "s1",
        type: "assistant" as const,
        content: [{ type: "text" as const, text: "alpha beta" }],
        created_at: new Date().toISOString(),
      },
    ];
    await tick();
  });

  await page.keyboard.press("ControlOrMeta+f");
  const input = page
    .getByRole("search", { name: "Search messages" })
    .getByRole("textbox", { name: "Search messages" });
  await input.fill("alpha");
  const rangeState = () =>
    page.evaluate(() => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const h = (CSS as any).highlights?.get("yomi-search-active");
      if (!h) return { text: null, connected: false };
      for (const r of h as Iterable<Range>) {
        return { text: r.toString(), connected: r.startContainer.isConnected };
      }
      return { text: null, connected: false };
    });
  await expect.poll(rangeState).toEqual({ text: "alpha", connected: true });

  // Simulate a stream frame that REPLACES the content (not append): the
  // renderer rebuilds its DOM, the old range's node is detached, and only
  // the re-pin effect paints a fresh range on the live node.
  await page.evaluate(async () => {
    const { tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    state.streamingMessages["search-stale-test"] = [
      {
        id: "s1",
        type: "assistant" as const,
        content: [{ type: "text" as const, text: "omega alpha beta" }],
        created_at: new Date().toISOString(),
      },
    ];
    await tick();
  });
  await expect.poll(rangeState).toEqual({ text: "alpha", connected: true });
});

test("rangeForOccurrence spans element-split text nodes", async ({ page }) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  const texts = await page.evaluate(() => {
    const { rangeForOccurrence } = window.__e2e.searchHighlight;
    const root = document.createElement("div");
    // "git" shredded across three element boundaries, then a plain hit.
    root.innerHTML = "<span>g</span><b>i</b><span>t</span> and git";
    const first = rangeForOccurrence(root, "git", 0);
    const second = rangeForOccurrence(root, "git", 1);
    const missing = rangeForOccurrence(root, "git", 2);
    return [
      first?.toString() ?? null,
      second?.toString() ?? null,
      missing?.toString() ?? null,
    ];
  });
  expect(texts).toEqual(["git", "git", null]);
});
