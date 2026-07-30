import { expect, test } from "@playwright/test";

test("discards a Mermaid render when the theme changes in flight", async ({
  page,
}) => {
  await page.route(/\/mermaid(?:\.js)?(?:\?|$)/, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await route.continue();
  });
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);

  const result = await page.evaluate(async () => {
    const { renderMermaid } = window.__e2e.mermaid;
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
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);

  const target = page.locator("#mermaid-lazy-test");
  await page.evaluate(async () => {
    const { mount } = window.__e2e.svelte;
    const { default: MermaidBlock } = window.__e2e.MermaidBlock;
    const scrollContainer = document.createElement("div");
    scrollContainer.style.height = "400px";
    scrollContainer.style.overflowY = "auto";
    const spacer = document.createElement("div");
    spacer.style.height = "1000px";
    const target = document.createElement("div");
    target.id = "mermaid-lazy-test";
    scrollContainer.append(spacer, target);
    document.body.replaceChildren(scrollContainer);
    mount(MermaidBlock, {
      target,
      props: { source: "graph TD; Lazy-->Rendered" },
    });
  });

  await expect(target).toContainText("Diagram ready when visible");
  await expect(target.locator(".mermaid-diagram svg")).toHaveCount(0);

  await target.scrollIntoViewIfNeeded();
  await expect(target.locator(".mermaid-diagram svg")).toHaveCount(1, {
    timeout: 10_000,
  });
});
