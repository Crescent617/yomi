import { expect, test, type Page } from "@playwright/test";

/**
 * E2E for the desktop-pet companion input (CompanionInput): IME-safe
 * Enter, slash-command completion and execution. The component is mounted
 * in isolation with a stub `onSend` and a stubbed `__TAURI_INTERNALS__` so
 * the real command path (shared with the main window) drives the real
 * invoke channel.
 */

const HOST = "#companion-input-host";
const TEXTAREA = `${HOST} textarea`;
const MENU = `${HOST} [class*="bottom-full"]`;

type E2EWindow = Window & {
  __companionSent: string[];
  __invokes: { cmd: string; args: Record<string, unknown> }[];
};

async function stubTauri(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as E2EWindow & {
      __TAURI_INTERNALS__: Record<string, unknown>;
    };
    w.__companionSent = [];
    w.__invokes = [];
    w.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        w.__invokes.push({ cmd, args: args ?? {} });
        if (cmd === "plugin:app|version") return "0.0.0-e2e";
        if (cmd === "plugin:store|load") return 1;
        if (cmd === "plugin:store|get") return [null, false];
        if (cmd === "plugin:store|set" || cmd === "plugin:store|save") {
          return null;
        }
        return null;
      },
      transformCallback: () => 0,
      unregisterCallback: () => {},
      plugins: {},
      convertFileSrc: (p: string) => p,
    };
  });
}

async function mountCompanionInput(page: Page) {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const { default: CompanionInput } = window.__e2e.CompanionInput;
    const w = window as unknown as E2EWindow;
    const host = document.createElement("div");
    host.id = "companion-input-host";
    document.body.appendChild(host);
    (window as unknown as { __cmp: unknown }).__cmp = mount(CompanionInput, {
      target: host,
      props: {
        session_id: "companion-e2e",
        disabled: false,
        placeholder: "Message…",
        onSend: async (text: string) => {
          w.__companionSent.push(text);
        },
      },
    });
    await tick();
  });
}

function sent(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__companionSent);
}

function invokes(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__invokes);
}

test("plain text sends via the panel channel and clears the draft", async ({
  page,
}) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("hello there");
  await textarea.press("Enter");

  await expect.poll(() => sent(page)).toEqual(["hello there"]);
  await expect(textarea).toHaveValue("");
});

test("typing / reveals the command menu and filters it", async ({ page }) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/");
  await expect(page.locator(MENU)).toBeVisible();
  await expect(
    page.locator(`${MENU} button`, { hasText: "/clear" }),
  ).toHaveCount(1);

  // A partial filter narrows the list.
  await textarea.fill("/cle");
  await expect(page.locator(`${MENU} button`)).toHaveCount(1);
  await expect(
    page.locator(`${MENU} button`, { hasText: "/clear" }),
  ).toHaveCount(1);

  // A space ends command-name entry and hides the menu.
  await textarea.fill("/clear ");
  await expect(page.locator(MENU)).toHaveCount(0);
});

test("Enter accepts the selected completion into the draft", async ({
  page,
}) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/cl");
  await textarea.press("Enter");
  await expect(textarea).toHaveValue("/clear ");
  // Accepting a completion does not send anything.
  expect(await sent(page)).toEqual([]);
});

test("executing an implemented command runs it on the bound session", async ({
  page,
}) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/clear");
  await expect(page.locator(MENU)).toBeVisible();
  // First Enter accepts the highlighted completion into the draft…
  await textarea.press("Enter");
  await expect(textarea).toHaveValue("/clear ");
  // …the second runs it.
  await textarea.press("Enter");

  // /clear is a session command: it hits the invoke channel for the bound
  // session instead of being sent as a message.
  await expect
    .poll(() => invokes(page))
    .toContainEqual({
      cmd: "clear_session",
      args: { session_id: "companion-e2e" },
    });
  expect(await sent(page)).toEqual([]);
  await expect(textarea).toHaveValue("");
});

test("an unknown command is sent as a normal message", async ({ page }) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/nosuchcmd do it");
  await textarea.press("Enter");

  await expect.poll(() => sent(page)).toEqual(["/nosuchcmd do it"]);
  expect(
    (await invokes(page)).some((c) => c.cmd === "clear_session"),
  ).toBeFalsy();
});

test("a slash with no match does not swallow Enter", async ({ page }) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  // "/nosuchcmd" (no space) matches no command, so the menu never shows and
  // Enter must fall through and send rather than being consumed silently.
  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/nosuchcmd");
  await expect(page.locator(MENU)).toHaveCount(0);
  await textarea.press("Enter");
  await expect.poll(() => sent(page)).toEqual(["/nosuchcmd"]);
});

test("/btw in the small window points at the main window", async ({ page }) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/btw why is the sky blue");
  await textarea.press("Enter");

  // The side-question card only renders in the main window; the small
  // window must neither silently run askBtw nor send the command as text.
  await expect(textarea).toHaveValue("");
  expect(await sent(page)).toEqual([]);
  expect((await invokes(page)).some((c) => c.cmd === "btw")).toBeFalsy();
  await expect(page.getByText("Side questions")).toBeVisible();
});

test("Escape closes the menu without dispatching a keydown upstream", async ({
  page,
}) => {
  await stubTauri(page);
  await mountCompanionInput(page);

  // A window-level listener stands in for the pet window's Escape handler
  // that closes the whole panel; it must NOT fire while the menu is open.
  await page.evaluate(() => {
    (window as unknown as { __escapes: number }).__escapes = 0;
    window.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        (window as unknown as { __escapes: number }).__escapes += 1;
      }
    });
  });

  const textarea = page.locator(TEXTAREA);
  await textarea.fill("/");
  await expect(page.locator(MENU)).toBeVisible();
  await textarea.press("Escape");
  await expect(page.locator(MENU)).toHaveCount(0);
  expect(
    await page.evaluate(
      () => (window as unknown as { __escapes: number }).__escapes,
    ),
  ).toBe(0);
});

test("Enter during IME composition does not send", async ({ page }) => {
  await stubTauri(page);
  await mountCompanionInput(page);
  await page.locator(TEXTAREA).click();

  // WebKit reports isComposing=false on the Enter that confirms a pinyin
  // candidate, so the guard must come from composition events, not the
  // flag. Simulate exactly that sequence.
  await page.evaluate(() => {
    const el = document.querySelector(
      "#companion-input-host textarea",
    ) as HTMLTextAreaElement;
    el.dispatchEvent(
      new CompositionEvent("compositionstart", { bubbles: true }),
    );
    el.value = "拼音";
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  expect(await sent(page)).toEqual([]);

  // The Enter that lands right after compositionend is swallowed too.
  await page.evaluate(() => {
    const el = document.querySelector(
      "#companion-input-host textarea",
    ) as HTMLTextAreaElement;
    el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    el.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  expect(await sent(page)).toEqual([]);

  // Past the swallow window a normal Enter finally sends the composed text.
  await page.waitForTimeout(150);
  await page.locator(TEXTAREA).press("Enter");
  await expect.poll(() => sent(page)).toEqual(["拼音"]);
});
