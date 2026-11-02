// 附件气泡 UI 截图（一次性产物，不进 CI 门禁）：
//   npx playwright test e2e/attachment-bubble-screenshots.spec.ts
// 输出 /tmp/attachment-shots/*.png（2x）。
import { test } from "@playwright/test";
import { mkdirSync } from "node:fs";

const OUT_DIR = "/tmp/attachment-shots";
mkdirSync(OUT_DIR, { recursive: true });

test.use({ deviceScaleFactor: 2, viewport: { width: 980, height: 660 } });

const NOW = "2026-11-02T12:40:00+08:00";
const LATER = "2026-11-02T12:41:30+08:00";

const MESSAGES = [
  {
    id: "u1",
    type: "user" as const,
    content: [
      {
        type: "text" as const,
        text: "把 FD meta 那几张表和 PRISMA 图整理好发我",
      },
    ],
    created_at: NOW,
  },
  {
    id: "a1",
    type: "assistant" as const,
    content: [
      { type: "text" as const, text: "整理好了，四页工作簿和流程图都在下面。" },
    ],
    created_at: LATER,
  },
  {
    id: "a2",
    type: "assistant" as const,
    content: [
      {
        type: "text" as const,
        text:
          "另外附上正文初稿和一份提取表说明：\n\n" +
          "<yomi_attachments>\n" +
          "/tmp/deliverables/manuscript_v10_BMC_revised.docx\n" +
          "88篇_数据提取表+RoB2+GRADE_儿童FD口服中药meta.xlsx\n" +
          "/tmp/deliverables/README.md\n" +
          "</yomi_attachments>",
      },
    ],
    created_at: LATER,
  },
];

async function boot(page: import("@playwright/test").Page) {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  await page.evaluate(async (messages) => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const { default: MessageList } = window.__e2e.MessageList;

    document.body.innerHTML = "";
    const shot = document.createElement("div");
    shot.id = "shot";
    shot.style.cssText =
      "box-sizing:border-box;width:760px;height:560px;padding:0;background:var(--background,#f7f5f0);" +
      "border:1px solid rgb(0 0 0 / 0.12);border-radius:10px;overflow:hidden;";
    document.body.appendChild(shot);

    state.sessionState.sessions.splice(0, state.sessionState.sessions.length);
    const session = sessionLib.createSessionState({
      id: "attachment-bubble-shot",
      messages: messages as never,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];
    mount(MessageList, { target: shot });
    await tick();
    await new Promise((r) => setTimeout(r, 250));
  }, MESSAGES);
}

test("attachment bubble shots", async ({ page }) => {
  await boot(page);

  // 1. 收起态：右侧 gutter 只多出一个回形针气泡（其余气泡无数据不显示）。
  await page.locator("#shot").screenshot({ path: `${OUT_DIR}/01-bubble.png` });

  // 2. 展开态：点开气泡，弹层里是会话交付的附件（可点开）。
  await page.locator("button[data-bubble='attachments']").click();
  await page.waitForTimeout(250);
  await page.locator("#shot").screenshot({ path: `${OUT_DIR}/02-panel.png` });
});
