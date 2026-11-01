// Extensions panel functional e2e (mocked Tauri IPC, real component):
//   npx playwright test e2e/ext-panel.spec.ts
// Complements e2e/ext-panel-screenshots.spec.ts (visual) with behavior.
import { expect, test } from "@playwright/test";

type Row = Record<string, unknown>;

async function boot(page: import("@playwright/test").Page, rows: Row[]) {
  await page.addInitScript(
    ({ rows }) => {
      const w = window as unknown as {
        __TAURI_INTERNALS__?: unknown;
        __invokeLog?: { cmd: string; args?: Record<string, unknown> }[];
      };
      w.__invokeLog = [];
      let live = rows;
      w.__TAURI_INTERNALS__ = {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          w.__invokeLog!.push({ cmd, args });
          if (cmd === "list_extensions") return live;
          if (cmd === "install_extension") {
            // The installed package now appears in the list.
            live = [
              ...live,
              {
                name: "demo",
                version: "0.1.0",
                description: "demo",
                dir: "/x/extensions/demo",
                health: "ok",
                source: "owner/repo",
                rev: "abcdef12",
                content_hash: "deadbeefcafe",
                installed_at: new Date().toISOString(),
                resources: { cron: [], hooks: [], bins: [], snippets: [] },
              },
            ];
            return {
              name: "demo",
              version: "0.1.0",
              content_hash: "deadbeefcafe",
              cron: [{ name: "ext:demo:hello", status: "created" }],
              hooks: [{ path: "hooks/turn_end/90-demo", status: "linked" }],
              bins: [{ path: "bin/demo-hello", status: "linked" }],
              init: null,
              snippets: ["convention.md"],
            };
          }
          if (cmd === "remove_extension") {
            live = live.filter((r) => r.name !== (args?.name as string));
            return {
              name: args?.name ?? "x",
              cron_removed: [],
              mounts_left: [],
              mounts_removed: ["bin/x"],
              ext_dir_removed: true,
            };
          }
          if (cmd === "open_default") return null;
          throw new Error(`unmocked invoke: ${cmd}`);
        },
      };
    },
    { rows },
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
      '<div id="host" class="bg-background text-foreground" style="display:flex;width:820px;height:640px"></div>';
    mount(w.__e2e.ExtensionsPanel.default as never, {
      target: document.querySelector("#host"),
    });
    await tick();
  });
}

const okRow: Row = {
  name: "memory-system",
  version: "0.1.0",
  description: "persistent memory",
  dir: "/x/extensions/memory-system",
  health: "ok",
  source: "Crescent617/yomi-extensions/ext/memory-system",
  rev: "1a2b3c4d5e6f",
  content_hash: "9f2a1b3c4d5e6f70",
  installed_at: "2026-11-01T00:00:00Z",
  resources: {
    cron: ["ext:memory-system:dream"],
    hooks: [],
    bins: ["recall"],
    snippets: [],
  },
};

test("renders installed extensions and reloads on Refresh", async ({
  page,
}) => {
  await boot(page, [okRow]);
  await mountPanel(page);
  await expect(page.getByText("memory-system", { exact: true })).toBeVisible();
  await expect(page.getByText("OK", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Refresh" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as { __invokeLog: { cmd: string }[] }
          ).__invokeLog.filter((e) => e.cmd === "list_extensions").length,
      ),
    )
    .toBeGreaterThanOrEqual(2);
});

test("empty state shows install guidance", async ({ page }) => {
  await boot(page, []);
  await mountPanel(page);
  await expect(page.getByText("No extensions installed")).toBeVisible();
  await expect(
    page.getByText("Crescent617/yomi-extensions/ext/memory-system"),
  ).toBeVisible();
});

test("unknown health renders a neutral 'Unknown state', not OK", async ({
  page,
}) => {
  await boot(page, [{ ...okRow, health: "future-variant" }]);
  await mountPanel(page);
  await expect(page.getByText("Unknown state")).toBeVisible();
  await expect(page.getByText("OK", { exact: true })).toHaveCount(0);
});

test("remove enumerates the target, then reloads", async ({ page }) => {
  await boot(page, [okRow]);
  await mountPanel(page);
  await page.getByRole("button", { name: "Remove" }).click();

  await expect(page.getByText("This deletes the package")).toBeVisible();
  await expect(page.getByText("ext:memory-system:dream")).toBeVisible();
  // The destructive confirm (dialog) is the only `bg-destructive` button.
  await page.locator("button.bg-destructive").click();

  await expect(page.getByText("No extensions installed")).toBeVisible();
  const args = await page.evaluate(() =>
    (
      window as unknown as {
        __invokeLog: { cmd: string; args?: { name?: string } }[];
      }
    ).__invokeLog
      .filter((e) => e.cmd === "remove_extension")
      .map((e) => e.args?.name),
  );
  expect(args).toEqual(["memory-system"]);
});

test("install dialog submits a source and shows the report", async ({
  page,
}) => {
  await boot(page, []);
  await mountPanel(page);
  await page
    .getByRole("button", { name: "Install", exact: true })
    .first()
    .click();
  await page.locator("#ext-source").fill("owner/repo");
  await page.locator("#ext-source").press("Enter");

  await expect(page.getByText("ext:demo:hello")).toBeVisible();
  await expect(page.getByRole("button", { name: "Done" })).toBeVisible();

  const args = await page.evaluate(() =>
    (
      window as unknown as {
        __invokeLog: { cmd: string; args?: { source?: string } }[];
      }
    ).__invokeLog
      .filter((e) => e.cmd === "install_extension")
      .map((e) => e.args?.source),
  );
  expect(args).toEqual(["owner/repo"]);

  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("button", { name: /^demo / })).toBeVisible();
});
