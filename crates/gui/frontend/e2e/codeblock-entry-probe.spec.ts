import { expect, test } from "@playwright/test";

/**
 * Reproduction probe for the field report: entering a session whose history
 * contains real fenced code blocks (raw pre → CodeBlock enhancement shrink →
 * async shiki highlight) intermittently fails to land/stay at the bottom.
 *
 * The probe drives the REAL rendering pipeline (streaming-markdown → rAF
 * enhancement → IO-gated shiki) and checks the pin at several checkpoints,
 * then probes pin liveness with an artificial growth.
 */
test("entry with real code blocks lands and stays at the bottom", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);

  const result = await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "codeblock-entry-test",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(session);
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const paragraph = (n: number) =>
      Array.from({ length: n }, (_, i) => `Paragraph line ${i}.`).join("\n\n");
    const code = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `const v${i} = f(${i});`).join(
        "\n",
      );
    // Long history with code blocks spread through it, ending with one near
    // the bottom (inside the entry viewport).
    const markdown = [
      paragraph(30),
      "```ts\n" + code(40) + "\n```",
      paragraph(30),
      "```python\n" + code(35) + "\n```",
      paragraph(25),
      "```rust\n" + code(30) + "\n```",
      paragraph(10),
    ].join("\n\n");

    live.messages.push(
      {
        id: "u1",
        type: "user",
        content: [{ type: "text", text: "review this" }],
        created_at: now,
      },
      {
        id: "a1",
        type: "assistant",
        content: [{ type: "text", text: markdown }],
        created_at: now,
      },
    );
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    const checkpoints: Record<string, number> = {};
    const snap = async (label: string, ms: number) => {
      await new Promise((resolve) => setTimeout(resolve, ms));
      checkpoints[label] = distance();
    };

    await snap("t100", 100);
    await snap("t400", 300);
    // Wait until shiki actually highlighted something (proves the async
    // pipeline ran), then keep sampling past it.
    const deadline = Date.now() + 15_000;
    while (
      !target.querySelector(".code-block .shiki span") &&
      Date.now() < deadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    checkpoints.afterHighlight = distance();
    await snap("tPlus600", 600);
    await snap("tPlus1500", 900);

    // Pin liveness probe: an artificial growth must be glued iff pinned.
    const content = target.querySelector<HTMLDivElement>(
      '[data-message-id="a1"]',
    );
    if (!content) throw new Error("message element not found");
    content.style.minHeight = `${content.offsetHeight + 800}px`;
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 120));
    checkpoints.afterProbeGrowth = distance();

    return {
      checkpoints,
      shikiBlocks: target.querySelectorAll(".code-block .shiki").length,
      codeBlocks: target.querySelectorAll(".code-block").length,
    };
  });

  console.log(
    "checkpoints:",
    result.checkpoints,
    "shiki:",
    result.shikiBlocks,
    "/",
    result.codeBlocks,
  );
  expect(result.codeBlocks).toBeGreaterThanOrEqual(3);
  for (const [label, d] of Object.entries(result.checkpoints)) {
    expect(d, `checkpoint ${label}`).toBeLessThanOrEqual(2);
  }
});
