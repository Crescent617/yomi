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

async function boot(
  page: Page,
  opts: {
    activeSessionId?: string;
    automationPageSize?: number;
    extraConversations?: number;
    extraAutomation?: number;
  } = {},
) {
  const extraConversations = Array.from(
    { length: opts.extraConversations ?? 0 },
    (_, i) =>
      row(
        `sess_extra_conv_${i}`,
        `older conversation ${i}`,
        "user",
        2000 + i * 30,
        PROJECT.id,
      ),
  );
  const extraAutomation = Array.from(
    { length: opts.extraAutomation ?? 0 },
    (_, i) =>
      row(`sess_extra_cron_${i}`, `cron run ${i}`, "cron", 1500 + i * 60),
  );

  await page.addInitScript(
    ({
      project,
      conversations,
      automation,
      activeSessionId,
      automationPageSize,
    }) => {
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
              const automationOnly =
                origins?.length === 2 && origins.includes("cron");
              const limit = automationOnly
                ? automationPageSize
                : ((args?.limit as number | undefined) ?? 50);
              const before = args?.before as string | undefined;
              let rows = [...conversations, ...automation].filter(
                (r) => !origins || origins.includes(r.origin),
              );
              // 与 kernel 同语义：先按 updated_at 倒序过滤游标，再截 limit。
              rows = rows.sort((a, b) =>
                a.updated_at < b.updated_at ? 1 : -1,
              );
              if (before) rows = rows.filter((r) => r.updated_at < before);
              const page = rows.slice(0, limit);
              const next_cursor =
                rows.length > limit ? page[page.length - 1].updated_at : null;
              return { sessions: page, next_cursor };
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
    {
      project: PROJECT,
      conversations: [...CONVERSATIONS, ...extraConversations],
      automation: [...AUTOMATION, ...extraAutomation],
      activeSessionId: opts.activeSessionId ?? null,
      automationPageSize: opts.automationPageSize ?? 10,
    },
  );

  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.evaluate(async (activeSessionId) => {
    const { mount, tick } = window.__e2e.svelte;
    const settings = window.__e2e.settings;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    settings.guiPreferences.layout.sidebar_view = "sessions";
    if (activeSessionId) {
      // 打开的就是一次 cron 运行：状态里带上来源，组件据此强制展开。
      state.sessionState.sessions.push(
        sessionLib.createSessionState({ id: activeSessionId, origin: "cron" }),
      );
      state.sessionState.activeSessionId = activeSessionId;
    }
    document.body.innerHTML =
      '<div id="sidebar-host" style="width:260px;height:100vh;display:flex;flex-direction:column;overflow:hidden"></div>';
    const target = document.querySelector<HTMLDivElement>("#sidebar-host");
    if (!target) throw new Error("missing sidebar host");
    mount(window.__e2e.ProjectSidebar.default, { target, props: {} });
    await tick();
  }, opts.activeSessionId ?? null);
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

  // 展开显示（第一页在进视图时就拉好了）
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

test("group auto-expands when the open session is an automation session", async ({
  page,
}) => {
  await boot(page, { activeSessionId: "sess_cron_1" });

  const header = page.getByRole("button", { name: "Automation" });
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(
    page.getByText("daily-task-recap-baishui · 2026-11-17 08:00:00"),
  ).toBeVisible();
  await page.waitForTimeout(250);
  await page.screenshot({ path: `${OUT_DIR}/03-auto-expanded.png` });

  // 显式收起：即便当前会话仍是自动任务，也应服从用户的点击。
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "false");
});

test("a full first page shows a count badge and loads more on demand", async ({
  page,
}) => {
  await boot(page, { automationPageSize: 3 });

  const header = page.getByRole("button", { name: "Automation" });
  // 4 条里只装了 3 条：徽标带 "+" 表示还有更多
  await expect(header).toContainText("3+");

  await header.click();
  const loadMore = page.getByRole("button", { name: "Load more" });
  await expect(loadMore).toBeVisible();
  await loadMore.click();

  await expect(
    page.getByText("daily-task-recap-baishui · 2026-11-16 08:00:00"),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${OUT_DIR}/04-load-more.png` });
});

test("the dock stays pinned to the sidebar bottom while lists scroll", async ({
  page,
}) => {
  await boot(page, { extraConversations: 30, extraAutomation: 15 });

  const header = page.getByRole("button", { name: "Automation" });
  await expect(header).toBeVisible();

  const dockBottom = async () =>
    page.evaluate(() => {
      const host = document.querySelector("#sidebar-host") as HTMLElement;
      const btn = [...document.querySelectorAll("button")].find((b) =>
        b.textContent?.includes("Automation"),
      );
      return {
        host: host.getBoundingClientRect().bottom,
        btn: btn?.getBoundingClientRect().bottom,
      };
    });

  const before = await dockBottom();
  expect(before.btn).toBeGreaterThan(before.host - 40);

  // 会话列表滚到底：Automation 仍贴在底部、位置不动
  await page.evaluate(() => {
    const scroller = document.querySelector(
      "#sidebar-host .scrollbar-hidden.flex-1",
    ) as HTMLElement | null;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  });
  await page.waitForTimeout(150);
  await expect(header).toBeVisible();
  const after = await dockBottom();
  expect(after.btn).toBeCloseTo(before.btn ?? 0, 0);

  // 展开后：内容区有内部滚动，且整块不超过侧栏高度的 45%
  await header.click();
  await page.waitForTimeout(250);
  const group = await page.evaluate(() => {
    const host = document.querySelector("#sidebar-host") as HTMLElement;
    const el = document.querySelector("#automation-sessions") as HTMLElement;
    const btn = [...document.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Automation"),
    ) as HTMLElement;
    return {
      inner: el.scrollHeight > el.clientHeight,
      dockHeight:
        host.getBoundingClientRect().bottom - btn.getBoundingClientRect().top,
      hostHeight: host.getBoundingClientRect().height,
    };
  });
  expect(group.inner).toBe(true);
  expect(group.dockHeight).toBeLessThanOrEqual(group.hostHeight * 0.46);
  await page.screenshot({ path: `${OUT_DIR}/05-docked-scroll.png` });
});
