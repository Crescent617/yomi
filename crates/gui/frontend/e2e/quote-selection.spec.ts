import { expect, test } from "@playwright/test";

test("quote popover: select transcript text → quote button → on_quote", async ({
  page,
}) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 640 });
  await page.emulateMedia({ reducedMotion: "reduce" });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;
    const { default: QuoteSelectionPopover } =
      window.__e2e.QuoteSelectionPopover;

    const created_at = new Date().toISOString();
    const session = sessionLib.createSessionState({
      id: "quote-test",
      messages: [
        {
          id: "query-1",
          type: "user" as const,
          content: [{ type: "text" as const, text: "Explain this code" }],
          created_at,
        },
        {
          id: "answer-1",
          type: "assistant" as const,
          content: [
            {
              type: "text" as const,
              text: "The function sorts the array in place and returns nothing.",
            },
          ],
          created_at,
        },
      ],
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];

    document.body.innerHTML =
      '<main id="quote-test-root" style="height:100vh;position:relative"></main><div id="quote-outside">unrelated text</div>';
    const target = document.querySelector<HTMLDivElement>("#quote-test-root");
    if (!target) throw new Error("Missing quote test target");
    mount(MessageList, { target });
    mount(QuoteSelectionPopover, {
      target: document.body,
      props: {
        on_quote: (text: string, message_id: string) => {
          (window as unknown as { __quote?: unknown }).__quote = {
            text,
            message_id,
          };
        },
      },
    });
    await tick();
  });

  const quoteButton = page.getByRole("button", {
    name: "Quote selection in composer",
  });
  await expect(quoteButton).toHaveCount(0);

  // Selection outside any message container: no button.
  await page.evaluate(() => {
    const outside = document.querySelector("#quote-outside");
    if (!outside?.firstChild) throw new Error("Missing outside node");
    const range = document.createRange();
    range.selectNodeContents(outside.firstChild);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
  });
  await expect(quoteButton).toHaveCount(0);

  // Selection inside an assistant message: button appears.
  await page.evaluate(() => {
    const p = document.querySelector(
      '[data-message-id="answer-1"] .text-block p',
    );
    if (!p) throw new Error("Missing assistant paragraph");
    const range = document.createRange();
    range.selectNodeContents(p);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
  });
  await expect(quoteButton).toBeVisible();

  await quoteButton.click();
  const quoted = await page.evaluate(
    () =>
      (window as unknown as { __quote?: { text: string; message_id: string } })
        .__quote,
  );
  expect(quoted?.text).toBe(
    "The function sorts the array in place and returns nothing.",
  );
  expect(quoted?.message_id).toBe("answer-1");

  // Button hides after accepting and the selection is cleared.
  await expect(quoteButton).toHaveCount(0);
  const selText = await page.evaluate(() => window.getSelection()?.toString());
  expect(selText).toBe("");

  // Selection spanning two messages: no button.
  await page.evaluate(() => {
    const query = document.querySelector('[data-message-id="query-1"]');
    const answer = document.querySelector('[data-message-id="answer-1"]');
    if (!query || !answer) throw new Error("Missing message nodes");
    const range = document.createRange();
    range.setStartBefore(query);
    range.setEndAfter(answer);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
  });
  await expect(quoteButton).toHaveCount(0);
});
