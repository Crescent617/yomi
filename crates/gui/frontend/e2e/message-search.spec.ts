import { expect, test } from "@playwright/test";

test("find in chat: ⌘F opens, navigates matches, highlights, closes", async ({
  page,
}) => {
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 640 });
  // Instant jumps: no smooth-scroll animation racing rapid key presses.
  await page.emulateMedia({ reducedMotion: "reduce" });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const created_at = new Date().toISOString();
    const messages = Array.from({ length: 6 }, (_, index) => [
      {
        id: `query-${index + 1}`,
        type: "user" as const,
        content: [
          { type: "text" as const, text: `Query ${index + 1}: inspect this` },
        ],
        created_at,
      },
      {
        id: `answer-${index + 1}`,
        type: "assistant" as const,
        content: [
          {
            type: "text" as const,
            text: "A detailed response. ".repeat(35),
          },
        ],
        created_at,
      },
    ]).flat();
    const session = sessionLib.createSessionState({
      id: "message-search-test",
      messages,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="message-search-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>(
      "#message-search-test",
    );
    if (!target) throw new Error("Missing search test target");
    mount(MessageList, { target });
    await tick();
  });

  const searchbar = page.getByRole("search", { name: "Search messages" });
  await expect(searchbar).toHaveCount(0);

  await page.keyboard.press("ControlOrMeta+f");
  await expect(searchbar).toBeVisible();
  const input = searchbar.getByRole("textbox", { name: "Search messages" });
  await expect(input).toBeFocused();

  await input.fill("inspect");
  const counter = searchbar.getByText(/^\d+\/\d+$/);
  await expect(counter).toHaveText("1/6");

  // The active match is registered with the CSS Custom Highlight API —
  // no DOM mutation, streaming-safe.
  await expect
    .poll(() =>
      page.evaluate(() => {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const registry = (CSS as any).highlights;
        return registry ? (registry.get("yomi-search-active")?.size ?? 0) : -1;
      }),
    )
    .toBe(1);

  // Typing jumped to the FIRST match in display order: query-1 sits in
  // the viewport (early messages clamp the scroll to 0, so no fixed
  // offset assertion here).
  await expect
    .poll(() =>
      page
        .locator('[data-message-id="query-1"]')
        .evaluate((element) => Math.round(element.getBoundingClientRect().top)),
    )
    .toBeGreaterThanOrEqual(0);

  await input.press("Enter");
  await expect(counter).toHaveText("2/6");

  // Deep matches must scroll: the jump lands them near the top of the
  // viewport (the e2e shell's own chrome shifts the exact pixel, so
  // assert the band, not the offset).
  await input.press("Enter");
  await input.press("Enter");
  await input.press("Enter");
  await expect(counter).toHaveText("5/6");
  await expect
    .poll(() =>
      page
        .locator('[data-message-id="query-5"]')
        .evaluate((element) => Math.round(element.getBoundingClientRect().top)),
    )
    .toBeGreaterThanOrEqual(0);
  const query5Top = await page
    .locator('[data-message-id="query-5"]')
    .evaluate((element) => Math.round(element.getBoundingClientRect().top));
  expect(query5Top).toBeLessThan(200);

  // Wrap-around: Enter past the last match returns to 1/6; Shift+Enter
  // from there wraps back to 6/6.
  await input.press("Enter");
  await expect(counter).toHaveText("6/6");
  await input.press("Enter");
  await expect(counter).toHaveText("1/6");
  await input.press("Shift+Enter");
  await expect(counter).toHaveText("6/6");

  // Editing the query after stepping resets to the first match (1/N),
  // never lands mid-list on a stale index.
  await input.fill("Query");
  await expect(counter).toHaveText("1/6");
  await expect
    .poll(() =>
      page
        .locator('[data-message-id="query-1"]')
        .evaluate((element) => Math.round(element.getBoundingClientRect().top)),
    )
    .toBeGreaterThanOrEqual(0);

  // A query with no matches shows 0/0 and clears the highlight.
  await input.fill("no-such-token");
  await expect(searchbar.getByText("0/0")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const registry = (CSS as any).highlights;
        return registry ? (registry.get("yomi-search-active")?.size ?? 0) : -1;
      }),
    )
    .toBe(0);

  await input.press("Escape");
  await expect(searchbar).toHaveCount(0);

  // ⌘F re-opens fresh.
  await page.keyboard.press("ControlOrMeta+f");
  await expect(searchbar).toBeVisible();
  await expect(input).toHaveValue("");
});
