import { beforeEach, describe, expect, test, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  forkSession: vi.fn(),
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
import {
  createSessionState,
  forkSession,
  imageUrlsFromBlocks,
  loadSessionMessages,
} from "./session";

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

describe("loadSessionMessages", () => {
  beforeEach(() => {
    sessionState.sessions.splice(0, sessionState.sessions.length);
  });

  test("marks the session history as loaded", () => {
    sessionState.sessions.push(createSessionState({ id: "s1" }));
    expect(sessionState.sessions[0].messages_loaded).toBe(false);

    loadSessionMessages("s1", []);

    expect(sessionState.sessions[0].messages_loaded).toBe(true);
  });

  test("skips replacing identical history to avoid a full re-render", () => {
    sessionState.sessions.push(createSessionState({ id: "s2" }));
    const history = [
      {
        id: "m1",
        kind: "user" as const,
        content: [{ type: "text" as const, text: "hi" }],
        created_at: "2026-07-28T00:00:00.000Z",
      },
    ];
    loadSessionMessages("s2", history);
    const session = sessionState.sessions[0];
    const rendered = session.messages;
    const revision = session.message_rewrite_revision;

    // A fresh payload with identical content must not trigger a replace.
    loadSessionMessages(
      "s2",
      history.map((message) => ({
        ...message,
        content: message.content.map((block) => ({ ...block })),
      })),
    );

    expect(session.messages).toBe(rendered);
    expect(session.message_rewrite_revision).toBe(revision);
  });

  test("replaces history when the fetched messages differ", () => {
    sessionState.sessions.push(createSessionState({ id: "s3" }));
    const created_at = "2026-07-28T00:00:00.000Z";
    loadSessionMessages("s3", [
      {
        id: "m1",
        kind: "user" as const,
        content: [{ type: "text" as const, text: "hi" }],
        created_at,
      },
    ]);
    const session = sessionState.sessions[0];
    const revision = session.message_rewrite_revision;

    loadSessionMessages("s3", [
      {
        id: "m1",
        kind: "user" as const,
        content: [{ type: "text" as const, text: "hi" }],
        created_at,
      },
      {
        id: "m2",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "hello" }],
        token_usage: null,
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ]);

    expect(session.messages).toHaveLength(2);
    expect(session.message_rewrite_revision).toBe(revision + 1);
  });

  test("clears stale token_usage when the reloaded history has none (#4)", () => {
    const session = createSessionState({ id: "s4" });
    sessionState.sessions.push(session);
    const created_at = "2026-07-28T00:00:00.000Z";
    const turn = [
      {
        id: "m1",
        kind: "user" as const,
        content: [{ type: "text" as const, text: "hi" }],
        created_at,
      },
      {
        id: "m2",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "hello" }],
        token_usage: {
          prompt_tokens: 900,
          completion_tokens: 100,
          total_tokens: 1000,
        },
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ];
    loadSessionMessages("s4", turn);
    expect(session.token_usage?.total_tokens).toBe(1000);

    // /clear: the history comes back empty — the pre-clear reading must
    // not survive, or the context gauge keeps showing it (#4).
    loadSessionMessages("s4", []);
    expect(session.token_usage).toBeUndefined();

    // /compact: the compactor strips per-message usage and rewrites the
    // history (new ids) — same clear-through-replace path.
    loadSessionMessages("s4", turn);
    loadSessionMessages("s4", [
      {
        id: "m3",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "summary" }],
        token_usage: null,
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ]);
    expect(session.token_usage).toBeUndefined();
  });

  test("identical history never wipes a live streaming token_usage", () => {
    const session = createSessionState({ id: "s6" });
    sessionState.sessions.push(session);
    loadSessionMessages("s6", []);
    // Mid-first-turn: a TokenUsage event landed; hydrating the same
    // (still empty) history must not clear it.
    session.token_usage = {
      prompt_tokens: 40,
      completion_tokens: 2,
      total_tokens: 42,
      context_window: 200_000,
    };
    loadSessionMessages("s6", []);
    expect(session.token_usage?.total_tokens).toBe(42);
  });

  test("keeps the latest assistant token_usage from the reload", () => {
    const session = createSessionState({ id: "s5" });
    sessionState.sessions.push(session);
    const created_at = "2026-07-28T00:00:00.000Z";
    loadSessionMessages("s5", [
      {
        id: "m1",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "a" }],
        token_usage: {
          prompt_tokens: 10,
          completion_tokens: 5,
          total_tokens: 15,
        },
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ]);
    expect(session.token_usage?.total_tokens).toBe(15);
  });

  test("reload inherits context_window from the live reading", () => {
    const session = createSessionState({ id: "s7" });
    sessionState.sessions.push(session);
    const created_at = "2026-07-28T00:00:00.000Z";
    const turn = [
      {
        id: "m1",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "a" }],
        token_usage: {
          prompt_tokens: 10,
          completion_tokens: 5,
          total_tokens: 15,
        },
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ];
    loadSessionMessages("s7", turn);
    // 流式事件补上了 effective window；历史里没有这个字段。
    session.token_usage = { ...session.token_usage!, context_window: 131_072 };
    // 下一轮历史（改写后）采用重载读数时，window 不得丢——丢了 gauge
    // 会隐藏到下一个 TokenUsage 事件。
    loadSessionMessages("s7", [
      ...turn,
      {
        id: "m2",
        kind: "assistant" as const,
        content: [{ type: "text" as const, text: "b" }],
        token_usage: {
          prompt_tokens: 20,
          completion_tokens: 6,
          total_tokens: 26,
        },
        response_id: null,
        model_id: null,
        finish_reason: null,
        created_at,
      },
    ]);
    expect(session.token_usage?.total_tokens).toBe(26);
    expect(session.token_usage?.context_window).toBe(131_072);
  });
});

describe("imageUrlsFromBlocks", () => {
  test("extracts live-event image blocks and keeps order", () => {
    const blocks = [
      { type: "image", url: "data:image/png;base64,AAA" },
      { type: "text", text: "[Image: a.png | Size: 3 bytes]" },
      {
        type: "image",
        url: "data:image/png;base64,BBB",
        mime_type: "image/png",
      },
    ];
    expect(imageUrlsFromBlocks(blocks)).toEqual([
      "data:image/png;base64,AAA",
      "data:image/png;base64,BBB",
    ]);
  });

  test("extracts persisted image_url blocks (history shape)", () => {
    const blocks = [
      {
        type: "image_url",
        image_url: { url: "asset://deadbeef.png" },
      },
      { type: "text", text: "meta" },
    ];
    expect(imageUrlsFromBlocks(blocks)).toEqual(["asset://deadbeef.png"]);
  });

  test("handles mixed shapes and non-arrays", () => {
    expect(
      imageUrlsFromBlocks([
        { type: "image", url: "data:image/png;base64,AAA" },
        { type: "image_url", image_url: { url: "asset://x.png" } },
      ]),
    ).toEqual(["data:image/png;base64,AAA", "asset://x.png"]);
    expect(imageUrlsFromBlocks("nope")).toEqual([]);
    expect(imageUrlsFromBlocks(undefined)).toEqual([]);
  });
});
