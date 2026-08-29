/**
 * Command palette state + command registry (VSCode Cmd+P / quick-open):
 * opened in session-search mode by default, `>` prefix (or Cmd+Shift+P)
 * switches to command mode. Pure filter helpers live at the bottom for
 * vitest; the Svelte component only renders `paletteState`.
 */
import type { ComponentType, SvelteComponent } from "svelte";
import {
  BarChart3,
  Bot,
  Bug,
  CalendarClock,
  Eraser,
  GitFork,
  MessageSquare,
  Plus,
  RotateCw,
  Settings,
  Star,
  SunMoon,
  Trash2,
} from "lucide-svelte";
import * as api from "./api";
import {
  projectState,
  purgeSessionLocalState,
  removeSessionAttentionItems,
  requestActivePanel,
  sessionState,
  showNotification,
  type SessionState,
} from "./state.svelte";
import {
  activateSession,
  createSessionState,
  forkSession,
  refreshSessions,
  setActiveSession,
} from "./session";
import {
  applyTheme,
  guiPreferences,
  scheduleGuiPreferencesSave,
} from "./settings.svelte";
import { fuzzyFilter } from "./fuzzy";

export interface PaletteCommand {
  id: string;
  group: "会话" | "内核" | "应用";
  title: string;
  /** Extra match text (english / pinyin) beyond `title`. */
  keywords: string;
  icon: ComponentType<SvelteComponent<{ class?: string }>>;
  /** Right-side mono hint ( consequence or shortcut ). */
  hint?: string;
  danger?: boolean;
  /** False → hidden in the current context (e.g. no active session). */
  enabled?: () => boolean;
  run: () => void | Promise<void>;
}

interface ConfirmRequest {
  title: string;
  message: string;
  confirmText: string;
  action: () => void | Promise<void>;
}

export const paletteState = $state({
  open: false,
  query: "",
  selected: 0,
  confirm: null as ConfirmRequest | null,
});

export function openPalette(commandMode: boolean) {
  paletteState.open = true;
  paletteState.query = commandMode ? ">" : "";
  paletteState.selected = 0;
  // Fresh session list for the goto view (fire-and-forget: rows fill in
  // when the fetch lands, the palette is already usable).
  refreshSessions();
}

export function closePalette() {
  paletteState.open = false;
  paletteState.query = "";
  paletteState.selected = 0;
}

/** `true` while the query carries the `>` command prefix. */
export function paletteInCommandMode(): boolean {
  return paletteState.query.startsWith(">");
}

// ── Actions ──────────────────────────────────────────────────────────

function reportFailure(what: string, e: unknown) {
  console.error(`${what}:`, e instanceof Error ? e.message : e);
  showNotification(`${what} 失败`, "error");
}

async function actionNewSession() {
  const project = projectState.projects.find(
    (p) => p.id === projectState.activeProjectId,
  );
  const working_dir = project?.dir ?? (await api.getCwd());
  const config = await api.getConfig();
  const level = config?.auto_approve ?? "caution";
  const id = await api.createSession(
    working_dir,
    level,
    project?.id,
    undefined,
  );
  sessionState.sessions.push(
    createSessionState({
      id,
      project_path: working_dir,
      project_id: project?.id,
      alias: "Untitled",
      permission_level: level,
    }),
  );
  await activateSession(id);
  requestActivePanel("chat");
}

async function actionForkActive() {
  const id = sessionState.activeSessionId;
  if (!id) return;
  const forked = await forkSession(id);
  await activateSession(forked.id);
  requestActivePanel("chat");
  showNotification("已 Fork 当前会话", "success");
}

async function actionClearActive() {
  const id = sessionState.activeSessionId;
  if (!id) return;
  await api.clearSession(id);
  showNotification("当前会话上下文已清空", "info");
}

async function actionDeleteSession(id: string) {
  try {
    await api.unsubscribe(id);
  } catch {
    // Not subscribed (never activated here) — deleting is still fine.
  }
  await api.deleteSession(id);
  sessionState.sessions = sessionState.sessions.filter((s) => s.id !== id);
  removeSessionAttentionItems(new Set([id]));
  purgeSessionLocalState(id);
  if (sessionState.activeSessionId === id) {
    const rest = sessionState.sessions;
    if (rest.length > 0) {
      await activateSession(rest[0].id);
    } else {
      setActiveSession(null);
    }
  }
  showNotification("会话已删除", "success");
}

function actionToggleTheme() {
  const order = ["light", "dark", "system"] as const;
  const idx = order.indexOf(guiPreferences.appearance.theme);
  const next = order[(idx + 1) % 3];
  guiPreferences.appearance.theme = next;
  applyTheme(next);
  scheduleGuiPreferencesSave();
  showNotification(
    `主题：${next === "light" ? "浅色" : next === "dark" ? "深色" : "跟随系统"}`,
    "info",
  );
}

function activeSessionTitle(): string {
  const id = sessionState.activeSessionId;
  if (!id) return "";
  const s = sessionState.sessions.find((item) => item.id === id);
  return s?.alias ?? id.slice(-8);
}

// ── Registry ─────────────────────────────────────────────────────────

export function paletteCommands(): PaletteCommand[] {
  const hasActive = () => sessionState.activeSessionId !== null;
  return [
    {
      id: "new-session",
      group: "会话",
      title: "新建会话",
      keywords: "new session create 新建 会话",
      icon: Plus,
      run: () => actionNewSession().catch((e) => reportFailure("新建会话", e)),
    },
    {
      id: "fork",
      group: "会话",
      title: "Fork 当前会话",
      keywords: "fork branch copy 复制 分支 会话",
      icon: GitFork,
      hint: "带全部上下文",
      enabled: hasActive,
      run: () => actionForkActive().catch((e) => reportFailure("Fork 会话", e)),
    },
    {
      id: "clear",
      group: "会话",
      title: "清空当前会话上下文",
      keywords: "clear context 清空 上下文 /clear",
      icon: Eraser,
      enabled: hasActive,
      run: () =>
        actionClearActive().catch((e) => reportFailure("清空上下文", e)),
    },
    {
      id: "delete",
      group: "会话",
      title: "删除当前会话…",
      keywords: "delete remove 删除 会话",
      icon: Trash2,
      hint: "不可恢复",
      danger: true,
      enabled: hasActive,
      run: () => {
        // Capture the target NOW: the confirm dialog can stay open while
        // the active session changes (notification click, rpc) — the
        // deletion must hit the session named in the message.
        const id = sessionState.activeSessionId;
        if (!id) return;
        paletteState.confirm = {
          title: "删除会话",
          message: `确定删除「${activeSessionTitle()}」吗？消息记录将一并删除，不可恢复。`,
          confirmText: "删除",
          action: () =>
            actionDeleteSession(id).catch((e) => reportFailure("删除会话", e)),
        };
      },
    },
    {
      id: "config",
      group: "内核",
      title: "编辑内核配置",
      keywords: "config kernel 配置 内核",
      icon: Settings,
      run: () => {
        requestActivePanel("config");
      },
    },
    {
      id: "restart",
      group: "内核",
      title: "重启 Kernel…",
      keywords: "restart reboot daemon kernel 重启 内核",
      icon: RotateCw,
      hint: "打断进行中的 run",
      danger: true,
      run: () => {
        paletteState.confirm = {
          title: "重启 Kernel",
          message: "重启会打断所有进行中的 run。确定重启吗？",
          confirmText: "重启",
          action: () =>
            api.restartDaemon().catch((e) => reportFailure("重启 Kernel", e)),
        };
      },
    },
    {
      id: "logs",
      group: "内核",
      title: "调试与日志",
      keywords: "debug logs 调试 日志",
      icon: Bug,
      run: () => {
        requestActivePanel("debug");
      },
    },
    {
      id: "usage",
      group: "应用",
      title: "用量统计",
      keywords: "usage token 用量 统计",
      icon: BarChart3,
      run: () => {
        requestActivePanel("usage");
      },
    },
    {
      id: "favorites",
      group: "应用",
      title: "收藏",
      keywords: "favorites star 收藏",
      icon: Star,
      run: () => {
        requestActivePanel("favorites");
      },
    },
    {
      id: "automation",
      group: "应用",
      title: "自动化",
      keywords: "automation cron 自动化 定时",
      icon: CalendarClock,
      run: () => {
        requestActivePanel("automation");
      },
    },
    {
      id: "agents",
      group: "应用",
      title: "Agents 面板",
      keywords: "agents subagent 面板",
      icon: Bot,
      run: () => {
        requestActivePanel("agents");
      },
    },
    {
      id: "theme",
      group: "应用",
      title: "切换主题",
      keywords: "theme dark light 主题 深色 浅色",
      icon: SunMoon,
      run: actionToggleTheme,
    },
  ];
}

// ── Pure filters (vitest targets) ────────────────────────────────────

/** Last path segment (`/a/b/c` → `c`); empty → empty. */
export function basename(path: string): string {
  return (
    path
      .split(/[\\/]+/)
      .filter(Boolean)
      .pop() ?? ""
  );
}

/** Enabled + fuzzy-filtered commands; `query` is already `>`-stripped. */
export function filterPaletteCommands(
  commands: readonly PaletteCommand[],
  query: string,
): PaletteCommand[] {
  const visible = commands.filter((c) => c.enabled?.() ?? true);
  return fuzzyFilter(query, visible, (c) => `${c.title} ${c.keywords}`);
}

/** Session rows for goto mode, most-recently-updated first. */
export function filterPaletteSessions(
  sessions: readonly SessionState[],
  query: string,
): SessionState[] {
  const sorted = [...sessions].sort((a, b) =>
    b.updated_at.localeCompare(a.updated_at),
  );
  return fuzzyFilter(
    query,
    sorted,
    (s) => `${s.alias ?? ""} ${s.project_path} ${s.id}`,
  ).slice(0, 50);
}

/** Shared row shape the component renders for both modes. */
export interface PaletteRow {
  key: string;
  group: string;
  title: string;
  hint?: string;
  icon: ComponentType<SvelteComponent<{ class?: string }>>;
  danger?: boolean;
  run: () => void;
}

export function commandRows(): PaletteRow[] {
  const q = paletteState.query.slice(1); // strip ">"
  const cmds = filterPaletteCommands(paletteCommands(), q);
  // fuzzyFilter 按分数重排会打散组连续性；按注册表组序重排（sort 稳定，
  // 组内仍是分数序）——分组 header 每组至多一次，keyed each 的
  // `h-<组名>` 才唯一（`>e` 这类跨组 query 必现重复 key）。
  const groupOrder = ["会话", "内核", "应用"];
  cmds.sort(
    (a, b) => groupOrder.indexOf(a.group) - groupOrder.indexOf(b.group),
  );
  return cmds.map((c) => ({
    key: c.id,
    group: c.group,
    title: c.title,
    hint: c.hint,
    icon: c.icon,
    danger: c.danger,
    run: () => void c.run(),
  }));
}

export function sessionRows(): PaletteRow[] {
  return filterPaletteSessions(sessionState.sessions, paletteState.query).map(
    (s) => ({
      key: s.id,
      group: "会话",
      title: s.alias ?? s.id.slice(-8),
      // id 尾段可见 = id 搜索能力可发现；目录只留 basename，长路径不挤。
      hint: `…${s.id.slice(-8)} · ${basename(s.project_path)}`,
      icon: MessageSquare,
      run: () => {
        void activateSession(s.id).catch(() => {
          // activateSession already notified the user.
        });
        requestActivePanel("chat");
      },
    }),
  );
}
