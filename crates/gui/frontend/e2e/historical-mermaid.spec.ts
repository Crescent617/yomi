import { expect, test } from "@playwright/test";

test("keeps a completed historical Mermaid mounted during streaming updates", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const events = await import("/src/lib/events.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    document.body.innerHTML = '<div id="message-list-test"></div>';
    const target = document.querySelector<HTMLDivElement>("#message-list-test");
    if (!target) throw new Error("Missing test target");

    const session = sessionLib.createSessionState({
      id: "historical-mermaid-streaming-test",
      messages: [
        {
          id: "historical-mermaid",
          type: "assistant" as const,
          content: [
            {
              type: "text" as const,
              text: "```mermaid\ngraph TD; A-->B\n```",
            },
          ],
          created_at: new Date().toISOString(),
        },
      ],
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];
    mount(MessageList, { target });

    await new Promise<void>((resolve, reject) => {
      const deadline = Date.now() + 10_000;
      const check = () => {
        if (target.querySelector(".mermaid-block svg")) resolve();
        else if (Date.now() >= deadline) reject(new Error("Mermaid timed out"));
        else requestAnimationFrame(check);
      };
      check();
    });

    const historicalBlock = target.querySelector(".mermaid-block");
    const samples = [];
    for (let index = 0; index < 20; index += 1) {
      events.handleEvent(session.id, `stream-${index}`, {
        model: {
          chunk: {
            message_id: "live-assistant",
            content: { text: "x" },
          },
        },
      });
      await tick();
      await new Promise(requestAnimationFrame);
      samples.push({
        sameBlock: target.querySelector(".mermaid-block") === historicalBlock,
        rawMermaidCode: target.querySelectorAll(
          "pre > code.mermaid, pre > code.language-mermaid",
        ).length,
      });
    }

    return samples;
  });

  expect(result).toEqual(
    Array.from({ length: 20 }, () => ({
      sameBlock: true,
      rawMermaidCode: 0,
    })),
  );
});
