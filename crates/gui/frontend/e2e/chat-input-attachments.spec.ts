import { expect, test, type Page } from "@playwright/test";

/**
 * E2E for ChatInput local-image attachment previews (the paperclip
 * flow). The native file dialog and fs reads are stubbed through
 * `window.__TAURI_INTERNALS__` — the dialog returns staged paths and fs
 * returns staged bytes — so the real ChatInput drives the real preview
 * pipeline: object-URL thumbnails, the app-shell lightbox, and silent
 * fallback to plain chips. The harness page only appends a host div, so
 * the layout's real ImagePreview instance serves the lightbox.
 */

// 1x1 transparent PNG.
const PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

const HOST = "#chatinput-host";
const ATTACH_BUTTON = `${HOST} button[title="Attach files"]`;
const THUMBS = `${HOST} img[src^="blob:"]`;

type E2EWindow = Window & {
  __dialogResult: string[] | null;
  __fsBytes: Record<string, number[]>;
  __fsRejected: string[];
  __urlStats: { created: number; revoked: number };
  __sentMessages: { session_id: string; content: string }[];
  __steers: {
    session_id: string;
    blocks: { type: string; text?: string }[];
  }[];
};

async function stubTauri(page: Page) {
  await page.addInitScript((pngB64: string) => {
    const w = window as unknown as E2EWindow & {
      __TAURI_INTERNALS__: Record<string, unknown>;
    };
    // Dialog staging: null = cancelled.
    w.__dialogResult = null;
    // fs staging: paths absent from this map fail the read (outside the
    // fs scope / unreadable); rejections are logged for assertions.
    w.__fsRejected = [];
    w.__fsBytes = {
      "/tmp/e2e/cat.png": Array.from(atob(pngB64), (c) => c.charCodeAt(0)),
      "C:\\pics\\dog.png": Array.from(atob(pngB64), (c) => c.charCodeAt(0)),
      "/tmp/e2e/fake.png": Array.from("not an image", (c) => c.charCodeAt(0)),
      "/tmp/e2e/icon.svg": Array.from(
        '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>',
        (c) => c.charCodeAt(0),
      ),
    };
    // Object-URL lifecycle spies: created proves a preview was attempted,
    // revoked proves cleanup ran (both are plain writable statics).
    w.__urlStats = { created: 0, revoked: 0 };
    w.__sentMessages = [];
    w.__steers = [];
    const origCreate = URL.createObjectURL.bind(URL);
    const origRevoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob: Blob | MediaSource) => {
      w.__urlStats.created++;
      return origCreate(blob);
    };
    URL.revokeObjectURL = (url: string) => {
      w.__urlStats.revoked++;
      origRevoke(url);
    };
    w.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "plugin:app|version") return "0.0.0-e2e";
        if (cmd === "plugin:store|load") return 1;
        if (cmd === "plugin:store|get") return [null, false];
        if (cmd === "plugin:store|set" || cmd === "plugin:store|save") {
          return null;
        }
        if (cmd === "plugin:dialog|open") return w.__dialogResult;
        if (cmd === "send_message") {
          w.__sentMessages.push(
            args as { session_id: string; content: string },
          );
          return null;
        }
        if (cmd === "send_steer") {
          w.__steers.push(
            args as {
              session_id: string;
              blocks: { type: string; text?: string }[];
            },
          );
          return null;
        }
        if (cmd === "mailbox_snapshot") return { queue: [], steer: [] };
        if (cmd === "plugin:fs|read_file") {
          const path = (args as { path: string }).path;
          const bytes = w.__fsBytes[path];
          if (!bytes) {
            w.__fsRejected.push(path);
            throw new Error(`fs read denied: ${path}`);
          }
          return bytes;
        }
        throw new Error(`unmocked invoke: ${cmd}`);
      },
      transformCallback: () => 0,
      unregisterCallback: () => {},
      plugins: {},
      convertFileSrc: (p: string) => p,
    };
  }, PNG_BASE64);
}

function urlStats(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__urlStats);
}

function fsRejected(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__fsRejected);
}

function sentMessages(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__sentMessages);
}

function steers(page: Page) {
  return page.evaluate(() => (window as unknown as E2EWindow).__steers);
}

function setDialogResult(page: Page, paths: string[] | null) {
  return page.evaluate(
    (result) => {
      (window as unknown as E2EWindow).__dialogResult = result;
    },
    paths as string[] | null,
  );
}

async function mountChatInput(page: Page, phase = "idle") {
  await page.goto("/e2e");
  // ssr=false route: the harness module runs after the shell load event.
  await page.waitForFunction(() => window.__e2e);
  await page.evaluate(async (initialPhase) => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: ChatInput } = window.__e2e.ChatInput;

    const session = sessionLib.createSessionState({
      id: "attach-e2e",
      phase: initialPhase,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    // Append (don't replace): the layout keeps ToastContainer and the
    // real ImagePreview lightbox alive alongside the mounted component.
    const host = document.createElement("div");
    host.id = "chatinput-host";
    document.body.appendChild(host);
    (window as unknown as { __cmp: unknown }).__cmp = mount(ChatInput, {
      target: host,
    });
    await tick();
  }, phase);
}

test("image attachments show thumbnails that open the lightbox", async ({
  page,
}) => {
  await stubTauri(page);
  await mountChatInput(page);

  // A cancelled dialog changes nothing.
  await page.locator(ATTACH_BUTTON).click();
  await expect(page.locator(THUMBS)).toHaveCount(0);

  await setDialogResult(page, ["/tmp/e2e/cat.png", "/tmp/e2e/notes.txt"]);
  await page.locator(ATTACH_BUTTON).click();

  // Only the image gets a thumbnail; the text file stays a plain chip.
  await expect(page.locator(THUMBS)).toHaveCount(1);
  await expect(
    page.locator(`${HOST} span`, { hasText: "notes.txt" }),
  ).toBeVisible();

  // Thumbnail click opens the app-shell lightbox; Escape closes it.
  await page
    .locator(`${HOST} button[title^="/tmp/e2e/cat.png"]`)
    .first()
    .click();
  const lightbox = page.getByRole("dialog", { name: "Image preview" });
  await expect(lightbox).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(lightbox).toHaveCount(0);

  // Removing the chip drops the thumbnail with it.
  await page
    .locator(
      `${HOST} div:has(> button[title^="/tmp/e2e/cat.png"]) > button[title="Remove"]`,
    )
    .click();
  await expect(page.locator(THUMBS)).toHaveCount(0);
  await expect(
    page.locator(`${HOST} span`, { hasText: "cat.png" }),
  ).toHaveCount(0);
});

test("undecodable image bytes fall back to a plain chip", async ({ page }) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/fake.png"]);
  await page.locator(ATTACH_BUTTON).click();

  // The preview pipeline ran (object URL created) but decoding failed,
  // so onerror dropped the thumbnail and revoked the URL; the chip stays.
  await expect.poll(() => urlStats(page)).toMatchObject({ created: 1 });
  await expect(page.locator(THUMBS)).toHaveCount(0);
  await expect
    .poll(() => urlStats(page))
    .toMatchObject({
      created: 1,
      revoked: 1,
    });
  await expect(
    page.locator(`${HOST} span`, { hasText: "fake.png" }),
  ).toBeVisible();
});

test("unreadable files keep the plain chip", async ({ page }) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/denied.png"]);
  await page.locator(ATTACH_BUTTON).click();

  // The read was rejected (logged by the fs stub) and no object URL was
  // ever created — the attachment stays a plain chip. The rejection is
  // asserted first; it strictly precedes any createObjectURL in
  // loadImagePreview, which is what makes the created:0 poll non-vacuous.
  await expect.poll(() => fsRejected(page)).toContain("/tmp/e2e/denied.png");
  await expect(page.locator(THUMBS)).toHaveCount(0);
  await expect.poll(() => urlStats(page)).toMatchObject({ created: 0 });
  await expect(
    page.locator(`${HOST} span`, { hasText: "denied.png" }),
  ).toBeVisible();
});

test("svg attachments get a thumbnail via the explicit blob type", async ({
  page,
}) => {
  await stubTauri(page);
  await mountChatInput(page);

  // Sniffing detects raster signatures but not markup: without the
  // component's explicit image/svg+xml blob type this thumbnail would
  // onerror-drop to a plain chip.
  await setDialogResult(page, ["/tmp/e2e/icon.svg"]);
  await page.locator(ATTACH_BUTTON).click();
  await expect(page.locator(THUMBS)).toHaveCount(1);
});

test("switching sessions clears attachments and revokes previews", async ({
  page,
}) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/cat.png"]);
  await page.locator(ATTACH_BUTTON).click();
  await expect(page.locator(THUMBS)).toHaveCount(1);

  // The ChatInput session-switch effect must clear the draft's
  // attachments and revoke their object URLs.
  await page.evaluate(async () => {
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    state.sessionState.sessions.push(
      sessionLib.createSessionState({ id: "attach-e2e-other" }),
    );
    state.sessionState.activeSessionId = "attach-e2e-other";
    await window.__e2e.svelte.tick();
  });
  await expect(page.locator(THUMBS)).toHaveCount(0);
  await expect(
    page.locator(`${HOST} span`, { hasText: "cat.png" }),
  ).toHaveCount(0);
  await expect
    .poll(() => urlStats(page))
    .toMatchObject({
      created: 1,
      revoked: 1,
    });
});

test("queued messages keep file attachments in the outgoing text", async ({
  page,
}) => {
  await stubTauri(page);
  // Streaming phase: Enter queues instead of sending. The queue path
  // shares the send path's text composer, so attachments ride along as
  // the [File: ...] suffix.
  await mountChatInput(page, "streaming");

  await setDialogResult(page, ["/tmp/e2e/notes.txt"]);
  await page.locator(ATTACH_BUTTON).click();
  await expect(
    page.locator(`${HOST} span`, { hasText: "notes.txt" }),
  ).toBeVisible();

  const textarea = page.locator(`${HOST} textarea`);
  await textarea.fill("queued hello");
  await textarea.press("Enter");

  await expect
    .poll(() => sentMessages(page))
    .toEqual([
      {
        session_id: "attach-e2e",
        content: "queued hello\n[File: /tmp/e2e/notes.txt]",
      },
    ]);
  // Queueing consumed the draft: attachment chips are cleared too.
  await expect(
    page.locator(`${HOST} span`, { hasText: "notes.txt" }),
  ).toHaveCount(0);
});

test("windows paths show a basename on the chip", async ({ page }) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["C:\\pics\\dog.png"]);
  await page.locator(ATTACH_BUTTON).click();

  // Exact text: a substring assertion would still pass if the chip
  // regressed to showing the full C:\pics\dog.png path.
  await expect(page.locator(`${HOST} span`, { hasText: "dog.png" })).toHaveText(
    "dog.png",
  );
  await expect(page.locator(THUMBS)).toHaveCount(1);
});

test("unmounting the component revokes pending previews", async ({ page }) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/cat.png"]);
  await page.locator(ATTACH_BUTTON).click();
  await expect(page.locator(THUMBS)).toHaveCount(1);

  // ChatInput unmounts when the session switches to a non-chat tab; its
  // onDestroy must revoke every outstanding preview URL.
  await page.evaluate(async () => {
    const { unmount, tick } = window.__e2e.svelte;
    const cmp = (window as unknown as { __cmp: Parameters<typeof unmount>[0] })
      .__cmp;
    await unmount(cmp);
    await tick();
  });
  await expect
    .poll(() => urlStats(page))
    .toMatchObject({
      created: 1,
      revoked: 1,
    });
});

test("/steer carries file attachments in the steer text", async ({ page }) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/notes.txt"]);
  await page.locator(ATTACH_BUTTON).click();
  await expect(
    page.locator(`${HOST} span`, { hasText: "notes.txt" }),
  ).toBeVisible();

  const textarea = page.locator(`${HOST} textarea`);
  await textarea.fill("/steer look at this");
  await textarea.press("Enter");

  // Same shared composer as send/queue: the steer text block ends with
  // the [File: ...] suffix, and the command path clears the chips.
  await expect
    .poll(() => steers(page))
    .toEqual([
      {
        session_id: "attach-e2e",
        blocks: [
          {
            type: "text",
            text: "look at this\n[File: /tmp/e2e/notes.txt]",
          },
        ],
      },
    ]);
  await expect(
    page.locator(`${HOST} span`, { hasText: "notes.txt" }),
  ).toHaveCount(0);
});

test("unknown commands treated as messages keep file attachments", async ({
  page,
}) => {
  await stubTauri(page);
  await mountChatInput(page);

  await setDialogResult(page, ["/tmp/e2e/notes.txt"]);
  await page.locator(ATTACH_BUTTON).click();

  const textarea = page.locator(`${HOST} textarea`);
  await textarea.fill("/nosuchcmd hello");
  await textarea.press("Enter");

  await expect
    .poll(() => sentMessages(page))
    .toEqual([
      {
        session_id: "attach-e2e",
        content: "/nosuchcmd hello\n[File: /tmp/e2e/notes.txt]",
      },
    ]);
});
