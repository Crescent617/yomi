import { expect, test } from "@playwright/test";

test("shows a skeleton while the initial message history loads", async ({
  page,
}) => {
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 640 });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const session = sessionLib.createSessionState({
      id: "message-loading-test",
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="message-loading-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>(
      "#message-loading-test",
    );
    if (!target) throw new Error("Missing loading test target");
    mount(MessageList, { target });
    await tick();
  });

  const skeleton = page.getByRole("status", { name: "Loading messages" });
  await expect(skeleton).toBeVisible();

  // Once the history arrives the skeleton is replaced by real messages.
  await page.evaluate(async () => {
    const sessionLib = window.__e2e.sessionLib;
    sessionLib.loadSessionMessages("message-loading-test", [
      {
        id: "query-1",
        kind: "user",
        content: [{ type: "text", text: "Hello skeleton" }],
        created_at: new Date().toISOString(),
      },
    ]);
    await window.__e2e.svelte.tick();
  });
  await expect(skeleton).toHaveCount(0);
  await expect(page.getByText("Hello skeleton")).toBeVisible();
});

test("renders cached messages immediately without a skeleton", async ({
  page,
}) => {
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 640 });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const created_at = new Date().toISOString();
    const session = sessionLib.createSessionState({
      id: "message-cached-test",
      messages_loaded: true,
      messages: [
        {
          id: "query-1",
          type: "user",
          content: [{ type: "text", text: "Cached hello" }],
          created_at,
        },
      ],
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="message-cached-test" style="height:100vh;position:relative"></main>';
    const target = document.querySelector<HTMLDivElement>(
      "#message-cached-test",
    );
    if (!target) throw new Error("Missing cached test target");
    mount(MessageList, { target });
    await tick();
  });

  await expect(page.getByText("Cached hello")).toBeVisible();
  await expect(
    page.getByRole("status", { name: "Loading messages" }),
  ).toHaveCount(0);
});
