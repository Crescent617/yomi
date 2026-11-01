// Extensions panel UI screenshots (one-off artifact, not a CI gate):
//   npx playwright test e2e/ext-panel-screenshots.spec.ts
// Outputs /tmp/ext-panel-shots/*.png (2x).
import { test } from "@playwright/test";
import { mkdirSync } from "node:fs";

const OUT_DIR = "/tmp/ext-panel-shots";
mkdirSync(OUT_DIR, { recursive: true });

test.use({ deviceScaleFactor: 2, viewport: { width: 820, height: 600 } });

async function boot(
  page: import("@playwright/test").Page,
  mode: "some" | "empty",
) {
  await page.addInitScript(
    ({ mode }) => {
      const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
      const now = Date.now();
      const rows =
        mode === "empty"
          ? []
          : [
              {
                name: "memory-system",
                version: "0.1.0",
                description: "Persistent memory for the agent",
                dir: "/Users/me/.yomi/extensions/memory-system",
                health: "ok",
                source: "Crescent617/yomi-extensions/ext/memory-system",
                rev: "1a2b3c4d5e6f7a8b9c0d",
                content_hash: "9f2a1b3c4d5e6f70",
                installed_at: new Date(now - 3 * 3600_000).toISOString(),
                resources: {
                  cron: [
                    "ext:memory-system:dream",
                    "ext:memory-system:janitor",
                  ],
                  hooks: [],
                  bins: ["recall"],
                  snippets: ["memory.md"],
                },
              },
              {
                name: "my-local-ext",
                version: "0.3.0",
                description: "Local dev package (edited on disk)",
                dir: "/Users/me/dev/my-ext",
                health: "modified",
                source: "/Users/me/dev/my-ext",
                installed_at: new Date(now - 26 * 3600_000).toISOString(),
                resources: {
                  cron: [],
                  hooks: ["pre_tool_use/50-guard"],
                  bins: [],
                  snippets: [],
                },
              },
              {
                name: "hand-placed",
                version: "0.0.1",
                description: "Dropped into extensions/ by hand",
                dir: "/Users/me/.yomi/extensions/hand-placed",
                health: "foreign",
              },
            ];
      w.__TAURI_INTERNALS__ = {
        invoke: async (cmd: string, args?: { name?: string }) => {
          if (cmd === "list_extensions") return rows;
          if (cmd === "install_extension")
            return {
              name: "demo",
              version: "0.1.0",
              content_hash: "9f2a1b3c",
              cron: [{ name: "ext:demo:hello", status: "created" }],
              hooks: [
                { path: "hooks/turn_end/90-demo-audit", status: "linked" },
              ],
              bins: [{ path: "bin/demo-hello", status: "linked" }],
              init: null,
              snippets: ["convention.md"],
            };
          if (cmd === "remove_extension")
            return {
              name: args?.name ?? "x",
              cron_removed: [],
              mounts_left: [],
              mounts_removed: ["bin/x"],
              ext_dir_removed: true,
            };
          if (cmd === "open_default") return null;
          throw new Error(`unmocked invoke: ${cmd}`);
        },
      };
    },
    { mode },
  );
  await page.goto("/e2e");
  await page.waitForFunction(
    () => (window as unknown as { __e2e?: unknown }).__e2e,
  );
}

async function mountPanel(page: import("@playwright/test").Page) {
  await page.evaluate(async () => {
    const w = window as unknown as {
      __e2e: {
        svelte: typeof import("svelte");
        ExtensionsPanel: { default: unknown };
      };
    };
    const { mount, tick } = w.__e2e.svelte;
    document.body.innerHTML =
      '<div id="host" class="bg-background text-foreground" style="display:flex;width:820px;height:600px"></div>';
    const target = document.querySelector("#host");
    if (!target) throw new Error("missing host");
    mount(w.__e2e.ExtensionsPanel.default as never, { target });
    await tick();
  });
}

test("list + detail", async ({ page }) => {
  await boot(page, "some");
  await mountPanel(page);
  await page.screenshot({ path: `${OUT_DIR}/1-list.png` });

  await page.locator("li").nth(1).locator("button").first().click();
  await page.screenshot({ path: `${OUT_DIR}/2-detail.png` });
});

test("install dialog", async ({ page }) => {
  await boot(page, "some");
  await mountPanel(page);
  await page.getByRole("button", { name: "Install", exact: true }).click();
  await page.screenshot({ path: `${OUT_DIR}/3-install.png` });
});

test("empty state", async ({ page }) => {
  await boot(page, "empty");
  await mountPanel(page);
  await page.screenshot({ path: `${OUT_DIR}/4-empty.png` });
});
