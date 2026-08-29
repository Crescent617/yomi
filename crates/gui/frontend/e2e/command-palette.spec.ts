import { expect, test } from "@playwright/test";

test("command palette: session search mode and command mode", async ({
  page,
}) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.setViewportSize({ width: 1280, height: 800 });

  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: CommandPalette } = window.__e2e.CommandPalette;

    const mk = (
      id: string,
      alias: string,
      project_path: string,
      updated_at: string,
    ) => {
      const s = sessionLib.createSessionState({
        id,
        project_path,
        alias,
      });
      s.updated_at = updated_at;
      state.sessionState.sessions.push(s);
    };
    mk(
      "sess_oct",
      "OCT meta 返修意见映射",
      "~/repos/meta_OCT_HM",
      "2026-08-29T08:00:00Z",
    );
    mk(
      "sess_gui",
      "GUI 流式渲染 O(delta) 化",
      "~/repos/yomi",
      "2026-08-29T12:30:00Z",
    );
    mk(
      "sess_kanban",
      "看板 skill 设计",
      "~/repos/yomi",
      "2026-08-29T11:00:00Z",
    );
    mk("sess_jmir", "JMIR 乳腺 meta R4", "~/work/jmir", "2026-08-28T09:00:00Z");
    state.sessionState.activeSessionId = "sess_gui";

    document.body.innerHTML =
      '<main id="palette-probe" style="height:100vh;background:var(--background)"></main>';
    const target = document.querySelector<HTMLElement>("#palette-probe");
    if (!target) throw new Error("missing probe target");
    mount(CommandPalette, { target });
    window.__e2e.commandPalette.openPalette(false);
    await tick();
  });

  // Session-search mode: grouped rows, most-recent first.
  const dialog = page.getByRole("dialog", { name: "命令面板" });
  await expect(dialog).toBeVisible();
  const options = page.getByRole("option");
  await expect(options).toHaveCount(4);
  await expect(options.first()).toContainText("GUI 流式渲染");
  await page.screenshot({ path: "e2e/out/palette-sessions.png" });

  // Keyboard navigation moves the selection ring.
  await page.keyboard.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");

  // Fuzzy filter narrows the session list.
  await page.keyboard.type("meta");
  await expect(options).toHaveCount(2);
  await expect(options.first()).toContainText("OCT meta");
  await page.screenshot({ path: "e2e/out/palette-sessions-filtered.png" });

  // Session id search: typing an id fragment finds the session by id,
  // and the row shows the id tail in the hint.
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Backspace");
  await page.keyboard.type("sess_kb");
  await expect(options).toHaveCount(1);
  await expect(options.first()).toContainText("看板 skill 设计");
  await expect(options.first()).toContainText("…s_kanban · yomi");
  await page.screenshot({ path: "e2e/out/palette-session-by-id.png" });

  // `>` prefix switches to command mode with icons and group labels.
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Backspace");
  await page.keyboard.type(">");
  await expect(options.first()).toContainText("新建会话");
  await expect(dialog).toContainText("内核");
  await expect(dialog).toContainText("重启 Kernel");
  await page.screenshot({ path: "e2e/out/palette-commands.png" });

  // Command-mode fuzzy filter, then Escape closes.
  await page.keyboard.type("restart");
  await expect(options).toHaveCount(1);
  await expect(options.first()).toContainText("重启 Kernel");

  // English keywords match Chinese-titled commands.
  await page.evaluate(() => {
    window.__e2e.commandPalette.paletteState.query = ">new";
  });
  await expect(options.first()).toContainText("新建会话");

  // Cross-group query (">e" matches 会话/内核/应用 alike): group headers
  // must stay contiguous — each group at most once (keyed-each safety).
  await page.evaluate(() => {
    window.__e2e.commandPalette.paletteState.query = ">e";
  });
  const headers = dialog.locator("p.micro-label");
  const headerTexts = await headers.allTextContents();
  expect(new Set(headerTexts).size).toBe(headerTexts.length);
  await page.screenshot({ path: "e2e/out/palette-commands-cross-group.png" });
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
});
