// 伙伴窗 UI 截图（一次性产物，不进 CI 门禁）：
//   npx playwright test e2e/companion-screenshots.spec.ts
// 输出 /tmp/companion-shots/*.png（2x）。
import { test } from "@playwright/test";
import { readFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const OUT_DIR = "/tmp/companion-shots";
mkdirSync(OUT_DIR, { recursive: true });

const SPRITESHEET_B64 = readFileSync(
  join(homedir(), ".yomi/pets/xiao-xiao-monthly-salary-meow/spritesheet.webp"),
).toString("base64");

const SESSION_ID = "companion-shot-session";

test.use({ deviceScaleFactor: 2 });

test.beforeEach(async ({ page }) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
});

/** 真实组件 + 真实 store 组装伙伴窗（展开=面板+精灵，收起=仅精灵）。 */
async function compose(
  page: import("@playwright/test").Page,
  options: {
    open: boolean;
    sprite_animation: string;
    sprite_scale?: number;
    sprite_x?: number;
    panel_height?: number;
    alert_dot?: "permission" | "ask_user" | null;
    mood?: string;
    messages?: Array<Record<string, unknown>>;
    streaming_text?: string;
    pending_permission?: Record<string, unknown> | null;
    backdrop: string;
  },
) {
  await page.evaluate(
    async ({ options: opts, spritesheet_b64, session_id }) => {
      const { mount, tick } = window.__e2e.svelte;
      const state = window.__e2e.state;
      const sessionLib = window.__e2e.sessionLib;
      const { companionState } = window.__e2e.companion;
      const { default: CompanionPanel } = window.__e2e.CompanionPanel;
      const { default: CodexPetSprite } = window.__e2e.CodexPetSprite;

      const scale = opts.sprite_scale ?? 1;
      const display_scale = opts.open ? scale * 0.7 : scale;

      // 绑定 session（走 override，等价于点击 pet 时的解析结果）。
      // 先清掉上一次 compose 的同 id session，避免 getSession 命中旧副本。
      state.sessionState.sessions.splice(0, state.sessionState.sessions.length);
      const session = sessionLib.createSessionState({
        id: session_id,
        alias: "Companion 设计走查",
      });
      state.sessionState.sessions.push(session);
      state.sessionState.activeSessionId = session.id;
      state.streamingMessages[session.id] = [];
      companionState.dto = {
        enabled: true,
        open: opts.open,
        pinned_session_id: null,
        hotkey: null,
      };
      companionState.open = opts.open;
      companionState.snapshot = {
        revision: 1,
        connection_status: "connected",
        running_count: 1,
        mood: (opts.mood ?? "working") as never,
        request:
          opts.alert_dot != null
            ? ({
                kind: opts.alert_dot,
                req_id: "req-1",
                session_id,
                title: "shell: rm -rf ./target",
              } as never)
            : null,
        notice: null,
        active_session_id: session_id,
      };
      companionState.override_session_id = session_id;
      // 与 Rust open_changed 携带的值保持一致（展开缩 0.7、锚点偏移）。
      companionState.sprite_scale = display_scale;
      companionState.sprite_x = opts.open ? (opts.sprite_x ?? 190) : null;

      if (opts.messages?.length) {
        sessionLib.loadSessionMessages(session_id, opts.messages as never);
      }
      if (opts.streaming_text) {
        session.phase = "streaming";
        state.streamingMessages[session.id] = [
          {
            id: "stream-1",
            type: "assistant",
            content: [{ type: "text", text: opts.streaming_text }],
            created_at: new Date().toISOString(),
          },
        ] as never;
        session.token_usage = {
          prompt_tokens: 12800,
          completion_tokens: 460,
          total_tokens: 13260,
          context_window: 262144,
        } as never;
      } else {
        session.phase = "idle";
      }
      if (opts.pending_permission) {
        session.pending_permissions.push(opts.pending_permission as never);
      }

      // 组装"窗口"：背景板 + （可选面板）+ 精灵区，布局复刻 pet 页模板。
      const bytes = Uint8Array.from(atob(spritesheet_b64), (c) =>
        c.charCodeAt(0),
      );
      const sprite_url = URL.createObjectURL(
        new Blob([bytes], { type: "image/webp" }),
      );

      document.body.innerHTML = "";
      const shot = document.createElement("div");
      shot.id = "shot";
      shot.style.cssText = `display:inline-block;padding:36px;background:${opts.backdrop};`;
      const frame = document.createElement("div");
      const panel_height = opts.panel_height ?? 560;
      frame.style.cssText =
        "display:flex;flex-direction:column;width:380px;" +
        (opts.open ? `height:${panel_height + 208 * display_scale}px;` : "");
      const spriteArea = document.createElement("div");
      spriteArea.style.cssText = `flex:none;height:${208 * display_scale}px;display:grid;place-items:center;position:relative;${opts.open ? "padding-top:8px;" : ""}`;
      const spriteBox = document.createElement("div");
      // 复刻 pet 页的锚点渲染：sprite_x 相对窗口中心的偏移。
      const shift_x = opts.open
        ? (opts.sprite_x ?? 190) - Math.max(380, 192 * display_scale) / 2
        : 0;
      spriteBox.style.cssText = `position:relative;width:${192 * display_scale}px;height:${208 * display_scale}px;${shift_x ? `transform:translateX(${shift_x}px);` : ""}`;
      spriteArea.appendChild(spriteBox);

      if (opts.open) {
        const panelWrap = document.createElement("div");
        panelWrap.style.cssText = "flex:1;min-height:0;display:flex;";
        frame.appendChild(panelWrap);
        frame.appendChild(spriteArea);
        shot.appendChild(frame);
        document.body.appendChild(shot);
        mount(CompanionPanel, { target: panelWrap });
      } else {
        frame.appendChild(spriteArea);
        shot.appendChild(frame);
        document.body.appendChild(shot);
      }
      mount(CodexPetSprite, {
        target: spriteBox,
        props: {
          src: sprite_url,
          animation: opts.sprite_animation,
          play_once: false,
          sprite_version_number: 2,
          look_index: null,
          scale: display_scale,
          restart_nonce: 0,
        },
      });
      if (opts.alert_dot) {
        const dot = document.createElement("span");
        dot.className =
          opts.alert_dot === "permission" ? "bg-error" : "bg-info";
        dot.style.cssText =
          "position:absolute;top:6%;right:10%;width:10px;height:10px;" +
          "border-radius:9999px;box-shadow:0 0 0 2px rgb(0 0 0 / 0.35);";
        spriteBox.appendChild(dot);
      }
      await tick();
    },
    { options, spritesheet_b64: SPRITESHEET_B64, session_id: SESSION_ID },
  );
}

const GRADIENT = "linear-gradient(160deg,#e8e4dc 0%,#cfd8e3 55%,#b9c4d6 100%)";

const NOW = "2026-10-16T21:40:00+08:00";
const LATER = "2026-10-16T21:41:12+08:00";

const CONVERSATION = [
  {
    id: "m1",
    kind: "user",
    content: [
      {
        type: "text",
        text: "帮我把伙伴窗面板的空状态文案写一下，要简短",
      },
    ],
    created_at: NOW,
  },
  {
    id: "m2",
    kind: "assistant",
    content: [
      {
        type: "text",
        text: "建议用这一句：\n\n> Nothing to follow yet. Activate a session in the main window, then click the pet again.\n\n理由是它同时覆盖了两种进入路径（主窗口激活、再点 pet），没有额外术语。更短的版本是 **“No session to follow yet.”**，但会丢掉引导动作。",
      },
    ],
    created_at: LATER,
    token_usage: {
      prompt_tokens: 8200,
      completion_tokens: 210,
      total_tokens: 8410,
    },
  },
];

test("companion shots", async ({ page }) => {
  // 1. 收起态：精灵 + 权限角标（alert=waiting 动画行）。
  await compose(page, {
    open: false,
    sprite_animation: "waiting",
    alert_dot: "permission",
    mood: "alert",
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/01-collapsed-alert.png` });

  // 2. 展开态：一轮完整对话 + 输入框（working=running 动画行）。
  await compose(page, {
    open: true,
    sprite_animation: "running",
    mood: "working",
    messages: CONVERSATION,
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/02-panel-conversation.png` });

  // 3. 展开态：流式中（shimmer 状态行 + 部分文本）。
  await compose(page, {
    open: true,
    sprite_animation: "running",
    mood: "working",
    messages: CONVERSATION.slice(0, 1),
    streaming_text:
      "空状态文案我建议用这一句：\n\n> Nothing to follow yet.\n\n它同时覆盖两种进入路径",
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/03-panel-streaming.png` });

  // 4. 展开态：待批权限（PermissionBar + alert 精灵 + 角标）。
  await compose(page, {
    open: true,
    sprite_animation: "waiting",
    alert_dot: "permission",
    mood: "alert",
    messages: CONVERSATION.slice(0, 1),
    pending_permission: {
      req_id: "req-1",
      session_id: SESSION_ID,
      tool_name: "shell",
      tool_args: JSON.stringify({ command: "rm -rf ./target" }),
      tool_level: "dangerous",
      reason: "删除构建产物目录以强制全量重建",
    },
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/04-panel-permission.png` });

  // 5. 展开态：窗口被右屏边夹住时，精灵按真实锚点渲染（不随窗口居中跑偏）。
  await compose(page, {
    open: true,
    sprite_animation: "idle",
    mood: "idle",
    sprite_x: 330,
    messages: CONVERSATION.slice(0, 1),
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/05-panel-clamped-right.png` });

  // 6. 展开态：内容少时面板收缩（高度自适应，min 240）。
  await compose(page, {
    open: true,
    sprite_animation: "idle",
    mood: "idle",
    panel_height: 300,
    messages: CONVERSATION.slice(0, 1),
    backdrop: GRADIENT,
  });
  await page
    .locator("#shot")
    .screenshot({ path: `${OUT_DIR}/06-panel-compact.png` });
});
