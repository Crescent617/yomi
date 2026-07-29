import { expect, test, type Page } from "@playwright/test";

/**
 * E2E for the StatusBar update prompt. Tauri IPC is stubbed through
 * `window.__TAURI_INTERNALS__` (getVersion -> 0.7.0, in-memory settings
 * store, recorded open_default calls) and the GitHub Releases API is
 * intercepted, so the real update-check module drives the real StatusBar UI.
 */

const RELEASE_API =
  "https://api.github.com/repos/Crescent617/yomi/releases/latest";

const releasePayload = (version: string) => ({
  tag_name: `v${version}`,
  html_url: `https://github.com/Crescent617/yomi/releases/tag/v${version}`,
  published_at: "2026-07-29T09:22:18Z",
});

async function stubTauri(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as {
      __invokeLog: { cmd: string; args?: unknown }[];
      __TAURI_INTERNALS__: Record<string, unknown>;
    };
    w.__invokeLog = [];
    w.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        w.__invokeLog.push({ cmd, args });
        if (cmd === "plugin:app|version") return "0.7.0";
        if (cmd === "plugin:store|load") return 1;
        if (cmd === "plugin:store|get") return [null, false];
        if (cmd === "plugin:store|set" || cmd === "plugin:store|save") {
          return null;
        }
        if (cmd === "open_default") return null;
        throw new Error(`unmocked invoke: ${cmd}`);
      },
      transformCallback: () => 0,
      unregisterCallback: () => {},
      plugins: {},
      convertFileSrc: (p: string) => p,
    };
  });
}

async function mountStatusBar(page: Page) {
  await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const { default: StatusBar } =
      await import("/src/lib/components/layout/StatusBar.svelte");
    document.body.innerHTML =
      '<div id="statusbar-host" style="position:fixed;left:0;right:0;bottom:0"></div>';
    const target = document.querySelector<HTMLDivElement>("#statusbar-host");
    if (!target) throw new Error("Missing statusbar host");
    mount(StatusBar, { target });
    await tick();
  });
}

async function runUpdateCheck(page: Page) {
  await page.evaluate(async () => {
    const mod = await import("/src/lib/update-check.svelte.ts");
    await mod.checkForUpdates();
  });
}

test("prompts for a newer release, opens it, and snoozes until a newer one", async ({
  page,
}) => {
  await stubTauri(page);
  await page.route(RELEASE_API, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(releasePayload("9.9.9")),
    }),
  );
  await page.goto("/");
  await mountStatusBar(page);

  // Up to date before any check: no prompt.
  await expect(page.getByText("v9.9.9", { exact: true })).toHaveCount(0);

  await runUpdateCheck(page);

  // Update chip appears in the status bar.
  const chip = page.getByTitle("Update available: v9.9.9 — click for details");
  await expect(chip).toBeVisible();
  await expect(page.getByText("v9.9.9", { exact: true })).toHaveCount(1);

  // Popover shows current -> latest and the publish date.
  await chip.click();
  await expect(
    page.getByText("Update available", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Current", { exact: true })).toBeVisible();
  await expect(page.getByText("Latest", { exact: true })).toBeVisible();
  await expect(page.getByText("v9.9.9", { exact: true })).toHaveCount(2);
  await expect(page.getByText(/^Published /)).toBeVisible();

  // "View release" forwards the release URL to the OS opener.
  await page.getByRole("button", { name: "View release" }).click();
  await expect
    .poll(async () =>
      page.evaluate(() =>
        (
          window as unknown as {
            __invokeLog: { cmd: string; args?: { target?: string } }[];
          }
        ).__invokeLog.filter((entry) => entry.cmd === "open_default"),
      ),
    )
    .toEqual([
      {
        cmd: "open_default",
        args: {
          target: "https://github.com/Crescent617/yomi/releases/tag/v9.9.9",
        },
      },
    ]);

  // Dismiss hides the chip and persists the snoozed version.
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(chip).toHaveCount(0);
  await expect(page.getByText("Update available", { exact: true })).toHaveCount(
    0,
  );
  await expect
    .poll(async () =>
      page.evaluate(
        async () =>
          (await import("/src/lib/settings.svelte.ts")).guiPreferences.updates
            .dismissed_version,
      ),
    )
    .toBe("9.9.9");

  // Re-checking the same release stays snoozed...
  await runUpdateCheck(page);
  await expect(page.getByText("v9.9.9", { exact: true })).toHaveCount(0);

  // ...but a newer release re-prompts.
  await page.unroute(RELEASE_API);
  await page.route(RELEASE_API, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(releasePayload("10.0.0")),
    }),
  );
  await runUpdateCheck(page);
  await expect(
    page.getByTitle("Update available: v10.0.0 — click for details"),
  ).toBeVisible();
});

test("stays silent when the release check fails", async ({ page }) => {
  await stubTauri(page);
  await page.route(RELEASE_API, (route) => route.abort());
  await page.goto("/");
  await mountStatusBar(page);

  await runUpdateCheck(page);

  await expect
    .poll(async () =>
      page.evaluate(
        async () =>
          (await import("/src/lib/update-check.svelte.ts")).updateCheckState
            .status,
      ),
    )
    .toBe("error");
  // No prompt, no error surface — the bar just shows the current version.
  await expect(page.getByTitle(/^Update available:/)).toHaveCount(0);
  await expect(page.getByText("v0.7.0", { exact: true })).toBeVisible();
});
