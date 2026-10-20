import { beforeEach, describe, expect, test, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getSession: vi.fn(),
  listRunningSessions: vi.fn(),
  listSubagents: vi.fn(),
  mailboxSnapshot: vi.fn(),
}));

vi.mock("./api", () => apiMocks);
vi.mock("@tauri-apps/plugin-notification", () => ({
  sendNotification: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-store", () => ({
  Store: { load: vi.fn(async () => ({ set: vi.fn(), save: vi.fn() })) },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

const listenMocks = vi.hoisted(() => ({
  callback: null as ((e: { payload: unknown }) => void) | null,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (_event: string, cb: (e: { payload: unknown }) => void) => {
      listenMocks.callback = cb;
      return () => {};
    },
  ),
}));

import {
  attentionItems,
  getSession as getLocalSession,
  runningSessions,
  sessionState,
  startNotificationListener,
  unreadSessions,
} from "./state.svelte";
import { createSessionState } from "./session";

function emit(payload: unknown) {
  listenMocks.callback?.({ payload });
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

async function resetState() {
  sessionState.sessions.splice(0, sessionState.sessions.length);
  sessionState.activeSessionId = null;
  attentionItems.splice(0, attentionItems.length);
  runningSessions.splice(0, runningSessions.length);
  for (const key of Object.keys(unreadSessions)) delete unreadSessions[key];
  const { mailboxBySession, pendingCounts } = await import("./mailbox.svelte");
  for (const key of Object.keys(mailboxBySession)) delete mailboxBySession[key];
  for (const key of Object.keys(pendingCounts)) delete pendingCounts[key];
  listenMocks.callback = null;
  vi.clearAllMocks();
  apiMocks.listRunningSessions.mockResolvedValue([]);
  apiMocks.listSubagents.mockResolvedValue([]);
  apiMocks.mailboxSnapshot.mockResolvedValue({ steer: [], queue: [] });
}

describe("startNotificationListener", () => {
  beforeEach(async () => {
    await resetState();
  });

  test("passive 模式照常刷新 phase 与运行列表（activity 气泡的数据源）", async () => {
    sessionState.sessions.push(createSessionState({ id: "s1" }));
    const unlisten = await startNotificationListener({ passive: true });
    // 启动即刷一次运行列表（修复前 passive 会跳过）。
    expect(apiMocks.listRunningSessions).toHaveBeenCalledTimes(1);

    // daemon 运行列表包含 s1——否则 reconcile 会把本地 active phase
    // 校正回 idle（运行列表是 phase 的权威源）。
    apiMocks.listRunningSessions.mockResolvedValue([
      {
        id: "s1",
        parent_id: null,
        title: null,
        project_id: null,
        phase: "streaming",
        background_task_count: 0,
        background_shells: [],
      },
    ]);
    emit({ state_changed: { session_id: "s1", status: "streaming" } });
    await flush();
    await flush();

    expect(getLocalSession("s1")?.phase).toBe("streaming");
    expect(apiMocks.listRunningSessions.mock.calls.length).toBeGreaterThan(1);
    expect(runningSessions.some((session) => session.id === "s1")).toBe(true);
    unlisten();
  });

  test("passive 模式跳过完成记录与未读标记", async () => {
    sessionState.sessions.push(createSessionState({ id: "s2" }));
    const unlisten = await startNotificationListener({ passive: true });

    emit({ state_changed: { session_id: "s2", status: "streaming" } });
    await flush();
    emit({ state_changed: { session_id: "s2", status: "idle" } });
    await flush();
    await flush();

    expect(unreadSessions["s2"]).toBeUndefined();
    expect(attentionItems.some((item) => item.sessionId === "s2")).toBe(false);
    unlisten();
  });

  test("非 passive 保持完成记录与未读标记", async () => {
    sessionState.sessions.push(createSessionState({ id: "s3" }));
    const unlisten = await startNotificationListener();

    emit({ state_changed: { session_id: "s3", status: "streaming" } });
    await flush();
    emit({ state_changed: { session_id: "s3", status: "idle" } });
    await flush();
    await flush();

    expect(unreadSessions["s3"]).toBe(true);
    expect(attentionItems.some((item) => item.sessionId === "s3")).toBe(true);
    unlisten();
  });

  test("passive 模式照常刷新 subagent 父会话（agents 气泡的数据源）", async () => {
    sessionState.sessions.push(createSessionState({ id: "s4" }));
    const unlisten = await startNotificationListener({ passive: true });
    apiMocks.getSession.mockResolvedValue({
      id: "sub_x",
      parent_id: "s4",
    });

    emit({ state_changed: { session_id: "sub_x", status: "streaming" } });
    await flush();
    await flush();

    expect(apiMocks.getSession).toHaveBeenCalledWith("sub_x");
    expect(apiMocks.listSubagents).toHaveBeenCalledWith("s4");
    unlisten();
  });

  test("passive 模式照常投递 mailbox 变更", async () => {
    sessionState.sessions.push(createSessionState({ id: "s5" }));
    const unlisten = await startNotificationListener({ passive: true });

    emit({ mailbox_changed: { session_id: "s5", steer: 1, queued: 2 } });
    await flush();

    const { pendingOf } = await import("./mailbox.svelte");
    expect(pendingOf("s5")).toBe(3);
    unlisten();
  });

  test("passive 模式下 sub_ 的标题变更照常刷新父会话", async () => {
    sessionState.sessions.push(createSessionState({ id: "s6" }));
    const unlisten = await startNotificationListener({ passive: true });
    apiMocks.getSession.mockResolvedValue({
      id: "sub_y",
      parent_id: "s6",
    });

    emit({ title_updated: { session_id: "sub_y", title: "子代理标题" } });
    await flush();
    await flush();

    expect(apiMocks.getSession).toHaveBeenCalledWith("sub_y");
    expect(apiMocks.listSubagents).toHaveBeenCalledWith("s6");
    unlisten();
  });

  test("passive 模式断连清空运行列表但不弹 toast", async () => {
    sessionState.sessions.push(createSessionState({ id: "s7" }));
    runningSessions.push({
      id: "s7",
      parent_id: null,
      title: null,
      project_id: null,
      phase: "streaming",
      background_task_count: 0,
      background_shells: [],
    });
    const { toasts } = await import("./toast.svelte");
    const toastCount = toasts.length;
    const unlisten = await startNotificationListener({ passive: true });

    emit({ connection_lost: { session_id: "s7" } });
    await flush();

    expect(runningSessions.length).toBe(0);
    expect(toasts.length).toBe(toastCount);
    unlisten();
  });
});
