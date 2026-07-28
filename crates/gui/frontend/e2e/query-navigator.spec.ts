import { expect, test } from "@playwright/test";

test("expands an independent user query navigator and jumps to queries", async ({
  page,
}) => {
  await page.goto("/");
  await page.setViewportSize({ width: 1280, height: 640 });

  await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

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
      id: "query-minimap-test",
      messages,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="query-minimap-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>(
      "#query-minimap-test",
    );
    if (!target) throw new Error("Missing minimap test target");
    mount(MessageList, { target });
    await tick();
  });

  const navigator = page.getByRole("navigation", {
    name: "User query navigator",
  });
  await expect(navigator).toBeVisible();
  await expect(navigator).toHaveAttribute("data-expanded", "false");
  const panel = navigator.locator(":scope > div").first();
  const collapsedPanelWidth = await panel.evaluate((element) =>
    Math.round(element.getBoundingClientRect().width),
  );
  await navigator.evaluate((element) => {
    element.setAttribute("data-instance", "before-streaming");
  });
  await page.evaluate(async () => {
    const events = await import("/src/lib/events.ts");
    for (let index = 0; index < 20; index += 1) {
      events.handleEvent("query-minimap-test", `token-${index}`, {
        model: {
          chunk: {
            message_id: "streaming-answer",
            content: { text: "token" },
          },
        },
      });
    }
    await (await import("/@id/svelte")).tick();
  });
  await expect(navigator).toHaveAttribute("data-instance", "before-streaming");

  const compactMarkers = navigator.getByRole("button", {
    name: /^Jump to query:/,
  });
  await expect(compactMarkers).toHaveCount(6);
  const markerTops = await compactMarkers.evaluateAll((markers) =>
    markers.map((marker) => Math.round(marker.getBoundingClientRect().top)),
  );
  expect(
    markerTops.slice(1).map((top, index) => top - markerTops[index]),
  ).toEqual(Array(5).fill(8));

  await navigator.dispatchEvent("mouseenter");
  await expect(navigator).toHaveAttribute("data-expanded", "true");
  await expect
    .poll(() =>
      panel.evaluate((element) =>
        Math.round(element.getBoundingClientRect().width),
      ),
    )
    .toBe(collapsedPanelWidth);
  await expect(page.getByText("User queries", { exact: true })).toHaveCount(0);
  await expect(page.getByText("6 turns", { exact: true })).toHaveCount(0);

  const secondQuery = navigator.getByRole("button", {
    name: "Query 2: inspect this",
    exact: true,
  });
  const beforeHover = await secondQuery.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  );
  await secondQuery.hover();
  await expect
    .poll(() =>
      secondQuery.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    )
    .not.toBe(beforeHover);
  await expect
    .poll(() =>
      secondQuery.evaluate((element) => getComputedStyle(element).transform),
    )
    .toBe("none");
  const list = navigator.locator(".query-navigator-list");
  await expect
    .poll(() =>
      list.evaluate((element) => getComputedStyle(element).scrollbarWidth),
    )
    .toBe("none");
  await secondQuery.click();
  await expect(secondQuery).toHaveAttribute("aria-current", "location");
  await expect
    .poll(() =>
      page
        .locator('[data-user-query-id="query-2"]')
        .evaluate((element) => Math.round(element.getBoundingClientRect().top)),
    )
    .toBe(16);

  await page.mouse.move(600, 300);
  await expect(navigator).toHaveAttribute("data-expanded", "false");

  await page.setViewportSize({ width: 600, height: 700 });
  await expect(navigator).toBeHidden();
});

test("caps the compact strip at 30 ticks and auto-scrolls to the active query", async ({
  page,
}) => {
  await page.goto("/");
  await page.setViewportSize({ width: 1280, height: 640 });

  await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const created_at = new Date().toISOString();
    const messages = Array.from({ length: 40 }, (_, index) => [
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
      id: "query-minimap-overflow-test",
      messages,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="query-minimap-overflow-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>(
      "#query-minimap-overflow-test",
    );
    if (!target) throw new Error("Missing minimap test target");
    mount(MessageList, { target });
    await tick();
  });

  const navigator = page.getByRole("navigation", {
    name: "User query navigator",
  });
  const strip = navigator.locator(".query-navigator-strip");
  const markers = navigator.getByRole("button", { name: /^Jump to query:/ });
  await expect(markers).toHaveCount(40);

  // The strip never grows past 30 ticks (30*6 + 29*2 + 8 = 246px).
  await expect
    .poll(() =>
      strip.evaluate((element) =>
        Math.round(element.getBoundingClientRect().height),
      ),
    )
    .toBe(246);

  // Every 10th tick is a slightly thicker line.
  const tickHeight = (nth: number) =>
    markers
      .nth(nth)
      .locator("span")
      .evaluate((element) =>
        Math.round(element.getBoundingClientRect().height),
      );
  await expect.poll(() => tickHeight(9)).toBe(4);
  expect(await tickHeight(19)).toBe(4);
  expect(await tickHeight(5)).toBe(2);

  const scrollChatTo = (top: number) =>
    page.evaluate((scrollTop) => {
      const container = document.querySelector<HTMLDivElement>(
        "main .h-full.overflow-y-auto",
      );
      if (!container) throw new Error("Missing scroll container");
      container.scrollTop = scrollTop;
      container.dispatchEvent(new Event("scroll"));
    }, top);

  // At the top of the chat the first query is active: no scrolling needed.
  await scrollChatTo(0);
  await expect
    .poll(() => strip.evaluate((element) => element.scrollTop))
    .toBe(0);

  // Scrolling the chat to the end moves the active query to the last one,
  // and the strip follows without any manual scrolling.
  await page.evaluate(() => {
    const container = document.querySelector<HTMLDivElement>(
      "main .h-full.overflow-y-auto",
    );
    if (!container) throw new Error("Missing scroll container");
    container.scrollTop = container.scrollHeight;
    container.dispatchEvent(new Event("scroll"));
  });
  await expect
    .poll(() => strip.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  await expect
    .poll(async () => {
      const stripBox = await strip.boundingBox();
      const lastBox = await markers.last().boundingBox();
      if (!stripBox || !lastBox) return false;
      return (
        lastBox.y >= stripBox.y - 1 &&
        lastBox.y + lastBox.height <= stripBox.y + stripBox.height + 1
      );
    })
    .toBe(true);
  const firstBox = await markers.first().boundingBox();
  const stripBox = await strip.boundingBox();
  expect(firstBox!.y).toBeLessThan(stripBox!.y);
});
