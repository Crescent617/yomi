import { expect, test } from "@playwright/test";

test.use({ viewport: { width: 980, height: 660 } });

test("clicking an attachment in the bubble popover opens the preview", async ({
  page,
}) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const host = document.createElement("div");
    host.id = "attach-host";
    host.style.cssText = "width:760px;height:560px;overflow:hidden;";
    document.body.appendChild(host);

    state.sessionState.sessions.splice(0, state.sessionState.sessions.length);
    const session = sessionLib.createSessionState({
      id: "attachment-click-test",
      messages: [
        {
          id: "a1",
          type: "assistant",
          content: [
            {
              type: "text",
              text: "附上说明：\n\n<yomi_attachments>\nREADME.md\nmanual.pdf\n</yomi_attachments>",
            },
          ],
          created_at: new Date().toISOString(),
        },
      ] as never,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];
    mount(MessageList, { target: host });
    await tick();
    await new Promise((r) => setTimeout(r, 200));
  });

  const bubble = page.locator("button[data-bubble='attachments']");
  await bubble.click();
  await expect(bubble).toHaveAttribute("aria-expanded", "true");

  // Scope to the popover panel (the sibling div the button owns), not the
  // whole page — otherwise `.last()` could silently fall back to the inline
  // chip in the message and the assertion would pass without the panel.
  const panel = page.locator("button[data-bubble='attachments'] ~ div");
  await expect(panel.getByRole("button", { name: "README.md" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "manual.pdf" })).toBeVisible();

  // README.md is a text type → in-app preview overlay.
  await panel.getByRole("button", { name: "README.md" }).click();
  await expect(
    page.locator('[role="dialog"][aria-label^="Preview "]'),
  ).toBeVisible();
});
