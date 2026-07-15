import { beforeEach, describe, expect, test, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  forkSession: vi.fn(),
  getGoal: vi.fn(),
  getMessages: vi.fn(),
  getSession: vi.fn(),
  getTodos: vi.fn(),
  listRunningSessions: vi.fn(),
  listSubagents: vi.fn(),
}));

vi.mock("./api", () => apiMocks);
vi.mock("@tauri-apps/plugin-notification", () => ({
  sendNotification: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

import { sessionState } from "./state.svelte";
import { createSessionState, forkSession } from "./session";

const hydratedSession = {
  id: "forked",
  phase: "idle",
  title: "Forked session",
  parent_id: null,
  project_id: "project",
  working_dir: "/workspace",
  message_count: 0,
  created_at: "2026-07-15T00:00:00.000Z",
  updated_at: "2026-07-15T00:00:00.000Z",
  auto_approve_level: null,
  model_key: null,
};

describe("forkSession", () => {
  beforeEach(() => {
    sessionState.sessions.splice(0, sessionState.sessions.length);
    sessionState.activeSessionId = null;
    vi.clearAllMocks();

    apiMocks.forkSession.mockResolvedValue("forked");
    apiMocks.getSession.mockResolvedValue(hydratedSession);
    apiMocks.getMessages.mockResolvedValue([]);
    apiMocks.getGoal.mockResolvedValue(null);
    apiMocks.getTodos.mockResolvedValue({ todos: [] });
    apiMocks.listRunningSessions.mockResolvedValue([]);
    apiMocks.listSubagents.mockResolvedValue([]);
  });

  test("inherits the parent permission level", async () => {
    sessionState.sessions.push(
      createSessionState({ id: "parent", permission_level: "dangerous" }),
    );

    await forkSession("parent");

    expect(apiMocks.forkSession).toHaveBeenCalledWith("parent", "dangerous");
  });

  test("uses an explicit permission override", async () => {
    sessionState.sessions.push(
      createSessionState({ id: "parent", permission_level: "dangerous" }),
    );

    await forkSession("parent", "safe");

    expect(apiMocks.forkSession).toHaveBeenCalledWith("parent", "safe");
  });

  test("removes the fork placeholder when hydration fails", async () => {
    const parent = createSessionState({
      id: "parent",
      permission_level: "safe",
    });
    sessionState.sessions.push(parent);
    apiMocks.getSession.mockRejectedValue(new Error("hydrate failed"));

    await expect(forkSession("parent")).rejects.toThrow("hydrate failed");

    expect(sessionState.sessions).toEqual([parent]);
    expect(
      sessionState.sessions.some((session) => session.id === "forked"),
    ).toBe(false);
  });
});
