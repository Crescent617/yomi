import { expect, test } from "@playwright/test";

/**
 * Switching from a tall session A to a code-block-heavy session B — the
 * real app flow. B's blocks go through raw-pre → enhancement shrink →
 * async highlight while the pin must hold.
 */
test("switching from a tall session to a code-block session lands at the bottom", async ({
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

    const now = new Date().toISOString();
    const paragraph = (n: number) =>
      Array.from({ length: n }, (_, i) => `Paragraph line ${i}.`).join("\n\n");
    const code = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `const v${i} = f(${i});`).join(
        "\n",
      );

    const sessionA = sessionLib.createSessionState({
      id: "switch-src",
      phase: "idle",
      messages_loaded: true,
    });
    const sessionB = sessionLib.createSessionState({
      id: "switch-dst",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(sessionA, sessionB);
    const liveA = state.sessionState.sessions.find((s) => s.id === sessionA.id);
    const liveB = state.sessionState.sessions.find((s) => s.id === sessionB.id);
    if (!liveA || !liveB) throw new Error("sessions not registered");

    liveA.messages.push(
      {
        id: "a-u1",
        type: "user",
        content: [{ type: "text", text: "plain" }],
        created_at: now,
      },
      {
        id: "a-a1",
        type: "assistant",
        content: [{ type: "text", text: paragraph(400) }],
        created_at: now,
      },
    );

    const markdown = [
      paragraph(30),
      "```ts\n" + code(40) + "\n```",
      paragraph(30),
      "```python\n" + code(35) + "\n```",
      paragraph(25),
      "```rust\n" + code(30) + "\n```",
      paragraph(10),
    ].join("\n\n");
    liveB.messages.push(
      {
        id: "b-u1",
        type: "user",
        content: [{ type: "text", text: "review this" }],
        created_at: now,
      },
      {
        id: "b-a1",
        type: "assistant",
        content: [{ type: "text", text: markdown }],
        created_at: now,
      },
    );

    state.sessionState.activeSessionId = sessionA.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 120));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const aAtBottom = distance();

    // The switch: B's raw pres render taller than B's enhanced blocks, the
    // enhancement shrink clamps, highlights complete in waves.
    state.sessionState.activeSessionId = sessionB.id;
    await tick();

    const checkpoints: Record<string, number> = {};
    const snap = async (label: string, ms: number) => {
      await new Promise((resolve) => setTimeout(resolve, ms));
      checkpoints[label] = distance();
    };
    await snap("t50", 50);
    await snap("t300", 250);
    const deadline = Date.now() + 15_000;
    while (
      !target.querySelector(".code-block .shiki span") &&
      Date.now() < deadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    checkpoints.afterHighlight = distance();
    await snap("tPlus800", 800);

    // Pin liveness probe.
    const messageEl = target.querySelector<HTMLElement>(
      '[data-message-id="b-a1"]',
    );
    if (!messageEl) throw new Error("message element not found");
    messageEl.style.minHeight = `${messageEl.offsetHeight + 800}px`;
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 120));
    checkpoints.afterProbeGrowth = distance();

    return {
      aAtBottom,
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
  expect(result.aAtBottom).toBeLessThanOrEqual(2);
  expect(result.codeBlocks).toBeGreaterThanOrEqual(3);
  for (const [label, d] of Object.entries(result.checkpoints)) {
    expect(d, `checkpoint ${label}`).toBeLessThanOrEqual(2);
  }
});
