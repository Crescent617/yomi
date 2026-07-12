import { expect, test } from "@playwright/test";

test("discards a Mermaid render when the theme changes in flight", async ({
  page,
}) => {
  await page.route(/\/mermaid(?:\.js)?(?:\?|$)/, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await route.continue();
  });
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { renderMermaid } = await import("/src/lib/mermaid.ts");
    const pending = renderMermaid("graph TD; A-->B").then(
      () => ({ resolved: true, name: "" }),
      (error: unknown) => ({
        resolved: false,
        name: error instanceof Error ? error.name : "",
      }),
    );

    await new Promise((resolve) => setTimeout(resolve, 50));
    document.documentElement.classList.add("dark");
    window.dispatchEvent(new CustomEvent("theme-changed"));
    return pending;
  });

  expect(result).toEqual({ resolved: false, name: "AbortError" });
});

test("renders a Mermaid block only when it approaches the viewport", async ({
  page,
}) => {
  await page.goto("/");

  const target = page.locator("#mermaid-lazy-test");
  await page.evaluate(async () => {
    const { mount } = await import("svelte");
    const { default: MermaidBlock } =
      await import("/src/lib/components/chat/MermaidBlock.svelte");
    const spacer = document.createElement("div");
    spacer.style.height = "10000px";
    const target = document.createElement("div");
    target.id = "mermaid-lazy-test";
    document.body.append(spacer, target);
    mount(MermaidBlock, {
      target,
      props: { source: "graph TD; Lazy-->Rendered" },
    });
  });

  await expect(target).toContainText("Diagram ready when visible");
  await expect(target.locator("svg")).toHaveCount(0);

  await target.scrollIntoViewIfNeeded();
  await expect(target.locator("svg")).toHaveCount(1, { timeout: 10_000 });
});
