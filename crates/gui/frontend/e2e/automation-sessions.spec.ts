import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

/**
 * Sidebar "Automation" group.
 *
 * Machine-produced sessions（cron per-run / ext_route）不再混在按时间排序的
 * 主列表里：它们收进列表末尾一个默认折叠的组。组标题只有一行，第一次
 * 展开才拉取内容；当前打开的会话本身是自动任务时强制展开。
 *
 * 数据从真实 api 模块走 `window.__TAURI_INTERNALS__.invoke` 桩；`list_sessions`
 * 桩按 `origins` 过滤，与 kernel 侧语义一致。
 *
 * 截图（非 CI 门槛）：npx playwright test e2e/automation-sessions.spec.ts
 * 输出 /tmp/sidebar-shots/*.png
 */

const OUT_DIR = "/tmp/sidebar-shots";
mkdirSync(OUT_DIR, { recursive: true });

test.use({ deviceScaleFactor: 2, viewport: { width: 300, height: 720 } });

const PROJECT = {
  id: "proj_yomi",
  name: "yomi",
  dir: "~/repos/yomi",
  created_at: "2026-10-01T00:00:00Z",
  updated_at: "2026-11-17T10:00:00Z",
};

const now = Date.now();
const row = (
  id: string,
  title: string,
  origin: string,
  minutesAgo: number,
  project_id: string | null = null,
) => ({
  id,
  working_dir: "~/repos/yomi",
  created_at: new Date(now - minutesAgo * 60_000).toISOString(),
  updated_at: new Date(now - minutesAgo * 60_000).toISOString(),
  title,
  project_id,
  auto_approve_level: "caution",
  message_count: 3,
  origin,
});

const CONVERSATIONS = [
  row("sess_conversation_1", "cron 会话独立或过滤", "user", 3, PROJECT.id),
  row("sess_conversation_2", "精读群内新文献", "user", 95, PROJECT.id),
  row("sess_conversation_3", "找现成IP池抓全市场", "channel", 1500, PROJECT.id),
];

const AUTOMATION = [
  row(
    "sess_cron_1",
    "daily-task-recap-baishui · 2026-11-17 08:00:00",
    "cron",
    5,
  ),
  row(
    "sess_cron_2",
    "ext:memory-system:janitor · 2026-11-17 05:55:00",
    "cron",
    200,
  ),
  row(
    "sess_cron_3",
    "ext:memory-system:dream · 2026-11-17 03:33:00",
    "cron",
    320,
  ),
  row(
    "sess_cron_4",
    "daily-task-recap-baishui · 2026-11-16 08:00:00",
    "cron",
    900,
  ),
];

async function boot(page: Page) {
  await page.addInitScript(
    ({ project, conversations, automation }) => {
      const w = window as unknown as {
        __invokeLog: { cmd: string; args?: Record<string, unknown> }[];
        __TAURI_INTERNALS__: Record<string, unknown>;
      };
      w.__invokeLog = [];
      w.__TAURI_INTERNALS__ = {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          w.__invokeLog.push({ cmd, args });
          switch (cmd) {
            case "list_projects":
              return [project];
            case "list_pinned_sessions":
              return [];
            case "list_sessions": {
              const origins = args?.origins as string[] | undefined;
              const rows = [...conversations, ...automation].filter(
                (r) => !origins || origins.includes(r.origin),
              );
              return { sessions: rows, next_cursor: null };
            }
            case "list_running_sessions":
              return [];
            case "refresh_mailbox":
              return { items: [] };
            case "get_session":
              return null;
            default:
              return null;
          }
        },
      };
    },
    { project: PROJECT, conversations: CONVERSATIONS, automation: AUTOMATION },
  );

  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const settings = window.__e2e.settings;
    settings.guiPreferences.layout.sidebar_view = "sessions";
    document.body.innerHTML =
      '<div id="sidebar-host" style="width:260px;height:100vh;display:flex;flex-direction:column;overflow:hidden"></div>';
    const target = document.querySelector<HTMLDivElement>("#sidebar-host");
    if (!target) throw new Error("missing sidebar host");
    mount(window.__e2e.ProjectSidebar.default, { target, props: {} });
    await tick();
  });
}

test("machine-produced sessions live in a collapsed Automation group", async ({
  page,
}) => {
  await boot(page);

  // 主列表只列人聊出来的会话
  await expect(page.getByText("cron 会话独立或过滤")).toBeVisible();
  await expect(page.getByText("精读群内新文献")).toBeVisible();
  await expect(
    page.getByText("daily-task-recap-baishui · 2026-11-17 08:00:00"),
  ).toHaveCount(0);

  const header = page.getByRole("button", { name: "Automation" });
  await expect(header).toBeVisible();
  await expect(header).toHaveAttribute("aria-expanded", "false");
  await page.screenshot({ path: `${OUT_DIR}/01-collapsed.png` });

  // 第一次展开才拉取内容
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(
    page.getByText("daily-task-recap-baishui · 2026-11-17 08:00:00"),
  ).toBeVisible();
  await page.waitForTimeout(250);
  await page.screenshot({ path: `${OUT_DIR}/02-expanded.png` });

  const automationCalls = await page.evaluate(() =>
    (
      window as unknown as { __invokeLog: { cmd: string; args?: unknown }[] }
    ).__invokeLog
      .filter((entry) => entry.cmd === "list_sessions")
      .map((entry) => (entry.args as { origins?: string[] })?.origins),
  );
  expect(automationCalls).toContainEqual(["user", "channel"]);
  expect(automationCalls).toContainEqual(["cron", "ext"]);

  // 再点收起
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "false");
});
