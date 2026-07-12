import { expect, test } from "@playwright/test";

test("renders a Mermaid fence as soon as it closes while streaming", async ({
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
    const target = document.createElement("div");
    target.style.height = "800px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "streaming-mermaid-close-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];
    mount(MessageList, { target });

    const send = (text: string, index: number) => {
      events.handleEvent(session.id, `stream-${index}`, {
        model: {
          chunk: {
            message_id: "live-assistant",
            content: { text },
          },
        },
      });
    };

    send("```mermaid\ngraph TD; A-->B", 0);
    await tick();
    await new Promise(requestAnimationFrame);
    const beforeClose = {
      blocks: target.querySelectorAll(".mermaid-block").length,
      rawCode: target.querySelectorAll("pre > code.mermaid").length,
    };

    send("\n```", 1);
    await tick();
    await new Promise<void>((resolve, reject) => {
      const deadline = Date.now() + 10_000;
      const check = () => {
        if (target.querySelector(".mermaid-block svg")) resolve();
        else if (Date.now() >= deadline) reject(new Error("Mermaid timed out"));
        else requestAnimationFrame(check);
      };
      check();
    });
    const renderedBlock = target.querySelector(".mermaid-block");

    send("\nMore streaming text", 2);
    await tick();
    await new Promise(requestAnimationFrame);
    return {
      beforeClose,
      afterClose: {
        blocks: target.querySelectorAll(".mermaid-block").length,
        rawCode: target.querySelectorAll("pre > code.mermaid").length,
        sameBlock: target.querySelector(".mermaid-block") === renderedBlock,
      },
    };
  });

  expect(result).toEqual({
    beforeClose: { blocks: 0, rawCode: 1 },
    afterClose: { blocks: 1, rawCode: 0, sameBlock: true },
  });
});
