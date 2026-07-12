import { expect, test } from "@playwright/test";

test("highlights a code fence once it closes and keeps it mounted", async ({
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
      id: "streaming-code-close-test",
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

    send("```ts\nconst value = 1;", 0);
    await tick();
    await new Promise(requestAnimationFrame);
    const beforeClose = {
      blocks: target.querySelectorAll(".code-block").length,
      rawCode: target.querySelectorAll("pre > code").length,
    };

    send("\n```", 1);
    await tick();
    await new Promise<void>((resolve, reject) => {
      const deadline = Date.now() + 10_000;
      const check = () => {
        if (target.querySelector(".code-block .shiki span")) resolve();
        else if (Date.now() >= deadline) reject(new Error("Shiki timed out"));
        else requestAnimationFrame(check);
      };
      check();
    });
    const renderedBlock = target.querySelector(".code-block");
    const renderedCode = target.querySelector(".code-block code");
    const heightBeforeTheme = renderedBlock?.getBoundingClientRect().height;

    send("\nMore streaming text", 2);
    await tick();
    await new Promise(requestAnimationFrame);
    document.documentElement.classList.add("dark");
    window.dispatchEvent(new Event("theme-changed"));
    await new Promise(requestAnimationFrame);

    return {
      beforeClose,
      afterClose: {
        blocks: target.querySelectorAll(".code-block").length,
        text: target.querySelector(".code-block code")?.textContent,
        sameBlock: target.querySelector(".code-block") === renderedBlock,
        sameCode: target.querySelector(".code-block code") === renderedCode,
        sameHeight:
          target.querySelector(".code-block")?.getBoundingClientRect()
            .height === heightBeforeTheme,
      },
    };
  });

  expect(result).toEqual({
    beforeClose: { blocks: 0, rawCode: 1 },
    afterClose: {
      blocks: 1,
      text: "const value = 1;",
      sameBlock: true,
      sameCode: true,
      sameHeight: true,
    },
  });
});

test("defers highlighting until a code block approaches the viewport", async ({
  page,
}) => {
  await page.goto("/");

  const target = page.locator("#lazy-code-test");
  await page.evaluate(async () => {
    const { mount } = await import("/@id/svelte");
    const { default: CodeBlock } =
      await import("/src/lib/components/chat/CodeBlock.svelte");
    const scrollContainer = document.createElement("div");
    scrollContainer.style.height = "400px";
    scrollContainer.style.overflowY = "auto";
    const spacer = document.createElement("div");
    spacer.style.height = "1000px";
    const target = document.createElement("div");
    target.id = "lazy-code-test";
    scrollContainer.append(spacer, target);
    document.body.replaceChildren(scrollContainer);
    mount(CodeBlock, {
      target,
      props: { code: "const lazy = true;", language: "typescript" },
    });
  });

  await expect(target.locator(".code-block-pre")).toContainText(
    "const lazy = true;",
  );
  await expect(target.locator(".shiki")).toHaveCount(0);

  await target.scrollIntoViewIfNeeded();
  await expect(target.locator(".shiki span").first()).toBeVisible({
    timeout: 10_000,
  });
});
