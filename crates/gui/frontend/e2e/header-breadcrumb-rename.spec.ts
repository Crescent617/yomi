import { expect, test, type Page } from "@playwright/test";

/**
 * E2E regression for the HeaderBreadcrumb "Rename current session" flow.
 *
 * Bug: clicking "Rename current session" swapped the button for the rename
 * input. Svelte flushes DOM updates synchronously after the event handler,
 * so the still-bubbling click reached the window-level closeMenus handler
 * with a target already detached from the DOM — `menuRef.contains(target)`
 * was false and the popover closed (the user had to reopen it to find the
 * input). The fix stopPropagation()s the click on both the Rename and Save
 * buttons, matching every other button in this component.
 *
 * Tauri IPC is stubbed through `window.__TAURI_INTERNALS__` (list_sessions
 * returns one session, rename_session is recorded) so the real api module
 * drives the real HeaderBreadcrumb UI.
 */

const SESSION_ID = "sess_rename_target";
const PROJECT_ID = "proj_yomi";

async function stubTauri(page: Page) {
  await page.addInitScript(
    ({ sessionId, projectId }) => {
      const w = window as unknown as {
        __invokeLog: { cmd: string; args?: unknown }[];
        __TAURI_INTERNALS__: Record<string, unknown>;
      };
      w.__invokeLog = [];
      w.__TAURI_INTERNALS__ = {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          w.__invokeLog.push({ cmd, args });
          if (cmd === "list_sessions") {
            return {
              sessions: [
                {
                  id: sessionId,
                  working_dir: "~/repos/yomi",
                  created_at: "2026-10-19T08:00:00Z",
                  updated_at: "2026-10-19T09:00:00Z",
                  title: "Old Name",
                  project_id: projectId,
                },
              ],
              next_cursor: null,
            };
          }
          if (cmd === "rename_session") return null;
          throw new Error(`unmocked invoke: ${cmd}`);
        },
        transformCallback: () => 0,
        unregisterCallback: () => {},
        plugins: {},
        convertFileSrc: (p: string) => p,
      };
    },
    { sessionId: SESSION_ID, projectId: PROJECT_ID },
  );
}

test("rename current session keeps the popover open", async ({ page }) => {
  await stubTauri(page);
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);

  await page.evaluate(
    async ({ sessionId, projectId }) => {
      const w = window as unknown as {
        __e2e: {
          svelte: typeof import("svelte");
          state: typeof import("../src/lib/state.svelte");
          sessionLib: typeof import("../src/lib/session");
          HeaderBreadcrumb: {
            default: import("svelte").Component<{
              session: import("../src/lib/state.svelte").SessionState;
            }>;
          };
        };
      };
      const { mount, tick } = w.__e2e.svelte;
      const session = w.__e2e.sessionLib.createSessionState({
        id: sessionId,
        project_path: "~/repos/yomi",
        project_id: projectId,
        alias: "Old Name",
      });
      w.__e2e.state.sessionState.sessions.push(session);
      document.body.innerHTML = '<div id="host"></div>';
      const target = document.querySelector<HTMLDivElement>("#host");
      if (!target) throw new Error("Missing host");
      mount(w.__e2e.HeaderBreadcrumb.default, {
        target,
        props: { session: w.__e2e.state.sessionState.sessions[0] },
      });
      await tick();
    },
    { sessionId: SESSION_ID, projectId: PROJECT_ID },
  );

  // Open the session menu from the breadcrumb.
  const crumb = page.getByRole("button", { name: /Old Name/ });
  await crumb.click();
  const renameEntry = page.getByRole("button", {
    name: "Rename current session",
  });
  await expect(renameEntry).toBeVisible();
  // Session list finished loading (crumb + list item both show the title).
  await expect(page.getByRole("button", { name: /Old Name/ })).toHaveCount(2);

  // Regression: clicking Rename must not close the popover.
  await renameEntry.click();
  const renameInput = page.locator("div.border-t input");
  await expect(renameInput).toBeVisible();
  await expect(renameInput).toHaveValue("Old Name");

  // Type a new name and save; the popover stays open throughout.
  await renameInput.fill("New Name");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (
          window as unknown as {
            __invokeLog: { cmd: string; args?: unknown }[];
          }
        ).__invokeLog.filter((entry) => entry.cmd === "rename_session"),
      ),
    )
    .toEqual([
      {
        cmd: "rename_session",
        args: { session_id: SESSION_ID, title: "New Name" },
      },
    ]);

  // Breadcrumb and still-open popover both reflect the new name.
  await expect(page.getByRole("button", { name: /New Name/ })).toHaveCount(2);
  await expect(
    page.getByRole("button", { name: "Rename current session" }),
  ).toBeVisible();
});
