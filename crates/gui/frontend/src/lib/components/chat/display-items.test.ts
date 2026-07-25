import { hasText } from "../../session";
import type {
  BotMessage,
  ErrorMessage,
  Message,
  SteerMessage,
  ToolMessage,
  UserMessage,
} from "../../state.svelte";
import { describe, expect, test } from "vitest";
import {
  buildDisplayItems,
  DisplayItemProjection,
  keyDisplayItems,
  liveActivityIndex,
} from "./display-items";

describe("message display item projection", () => {
  test("matches the full builder across stable and streaming transitions", () => {
    const created_at = "2026-01-01T00:00:00.000Z";
    const user = (id: string, text: string): UserMessage => ({
      id,
      type: "user" as const,
      content: [{ type: "text", text }],
      created_at,
    });
    const steer = (id: string, text: string): SteerMessage => ({
      id,
      type: "steer" as const,
      content: [{ type: "text", text }],
      created_at,
    });
    const assistant = (
      id: string,
      content: BotMessage["content"],
      tool_calls?: BotMessage["tool_calls"],
    ): BotMessage => ({
      id,
      type: "assistant" as const,
      content,
      tool_calls,
      created_at,
    });
    const tool = (
      id: string,
      status: ToolMessage["status"] = "running",
    ): ToolMessage => ({
      id,
      type: "tool" as const,
      tool_call_id: `${id}-call`,
      tool_name: "read",
      status,
      arguments: "{}",
      result: [],
      created_at,
    });
    const error = (id: string): ErrorMessage => ({
      id,
      type: "error" as const,
      content: `error-${id}`,
      created_at,
    });

    const cache = new DisplayItemProjection();
    let stable: Message[] = [user("u1", "start")];
    let stream: Message[] = [];
    const results: Array<{ label: string; equal: boolean }> = [];

    const normalize = (items: ReturnType<typeof buildDisplayItems>) =>
      items.map((item) =>
        item.type === "message"
          ? {
              type: item.type,
              ids: [item.message.id],
              isStreaming: item.isStreaming,
            }
          : {
              type: item.type,
              ids: item.messages.map((message) => message.id),
              ...(item.type === "action_group"
                ? { isStreaming: item.isStreaming }
                : {}),
            },
      );

    const compare = (label: string, streaming: boolean) => {
      const sections = cache.update("session-a", stable, 0, stream, streaming);
      const stableIds = new Set(stable.map((message) => message.id));
      const effectiveStream = stream.filter(
        (message) => !stableIds.has(message.id),
      );
      const stableTail = stable.at(-1);
      const stableTailIsOpen =
        stableTail?.type === "tool" ||
        stableTail?.type === "error" ||
        (stableTail?.type === "assistant" && !hasText(stableTail.content));
      const fullMessages = [...stable, ...effectiveStream];
      results.push({
        label,
        equal:
          JSON.stringify(
            normalize([...sections.stableItems, ...sections.dynamicItems]),
          ) ===
          JSON.stringify(
            normalize(
              buildDisplayItems(
                fullMessages,
                streaming && (effectiveStream.length > 0 || stableTailIsOpen),
              ),
            ),
          ),
      });
    };

    compare("user boundary", false);

    const streamingAssistant = assistant("a1", [
      { type: "thinking", thinking: "hmm" },
    ]);
    stream = [streamingAssistant];
    compare("thinking", true);

    streamingAssistant.content.push({ type: "text", text: "calling" });
    streamingAssistant.tool_calls = [
      { id: "call-1", name: "read", arguments: "{}" },
    ];
    const streamingTool = tool("t1");
    stream.push(streamingTool);
    compare("thinking tool and text", true);

    streamingTool.status = "completed";
    stream.push(assistant("a2", [{ type: "text", text: "done" }]));
    compare("plain streaming text", true);

    stable = [...stable, ...stream];
    stream = [];
    compare("committed stream", false);

    stable = [...stable, error("e1"), error("e2")];
    compare("trailing errors", false);

    const stableTool = tool("stable-tool");
    stable.push(stableTool);
    compare("stable open tool", true);
    stableTool.status = "completed";
    compare("mutated stable open tool", true);

    stream = [steer("s1", "redirect")];
    compare("steer boundary", true);

    stable = [...stable, ...stream];
    stream = [assistant("dup", [{ type: "text", text: "stable wins" }])];
    stable = [
      ...stable,
      assistant("dup", [{ type: "text", text: "committed" }]),
    ];
    compare("stable duplicate stream id", true);

    stream = [
      assistant("stream-dup", [{ type: "text", text: "one" }]),
      assistant("stream-dup", [{ type: "text", text: "two" }]),
    ];
    compare("duplicates inside stream remain", true);

    expect(results).toEqual(
      results.map(({ label }) => ({ label, equal: true })),
    );
  });

  test("preserves stable item references across streaming and append-only updates", () => {
    const created_at = "2026-01-01T00:00:00.000Z";
    const user = {
      id: "user-1",
      type: "user" as const,
      content: [{ type: "text", text: "start" }],
      created_at,
    };
    const assistant = {
      id: "assistant-1",
      type: "assistant" as const,
      content: [{ type: "text", text: "done" }],
      created_at,
    };
    const streaming = {
      id: "assistant-stream",
      type: "assistant" as const,
      content: [{ type: "text", text: "working" }],
      created_at,
    };
    const cache = new DisplayItemProjection();

    const initial = cache.update("session-a", [user], 0, [], false);
    const initialItems = initial.stableItems;
    expect(initialItems).toHaveLength(1);
    expect(initialItems[0]).toMatchObject({
      type: "message",
      message: user,
    });

    const duringStream = cache.update(
      "session-a",
      [user],
      0,
      [streaming],
      true,
    );
    expect(duringStream.stableItems).toBe(initialItems);

    const afterAppend = cache.update(
      "session-a",
      [user, assistant],
      0,
      [],
      false,
    );
    expect(afterAppend.stableItems).not.toBe(initialItems);
    expect(afterAppend.stableItems).toHaveLength(2);

    const appendedItems = afterAppend.stableItems;
    const nextStream = cache.update(
      "session-a",
      [user, assistant],
      0,
      [streaming],
      true,
    );
    expect(nextStream.stableItems).toBe(appendedItems);
  });

  test("invalidates on rewrite, shrink, and session switch", () => {
    const created_at = "2026-01-01T00:00:00.000Z";
    const message = (id: string, type: "user" | "steer" = "user") => ({
      id,
      type,
      content: [{ type: "text", text: id }],
      created_at,
    });
    const ids = (items: ReturnType<typeof buildDisplayItems>) =>
      items.flatMap((item) =>
        item.type === "message"
          ? item.message.id
          : item.messages.map((entry) => entry.id),
      );
    const cache = new DisplayItemProjection();
    let revision = 0;
    const update = (session: string, stable: ReturnType<typeof message>[]) => {
      const sections = cache.update(session, stable, revision, [], false);
      const optimized = ids([
        ...sections.stableItems,
        ...sections.dynamicItems,
      ]);
      const full = ids(buildDisplayItems(stable, false));
      return JSON.stringify(optimized) === JSON.stringify(full)
        ? optimized
        : ["cache-mismatch"];
    };

    const original = [message("a"), message("b", "steer")];
    const first = update("session-a", original);
    revision += 1;
    const replacement = update("session-a", [message("x"), message("y")]);
    revision += 1;
    const shrunk = update("session-a", [message("x")]);
    const switched = update("session-b", [message("z")]);

    expect({ first, replacement, shrunk, switched }).toEqual({
      first: ["a", "b"],
      replacement: ["x", "y"],
      shrunk: ["x"],
      switched: ["z"],
    });
  });
});

test("invalidates cached groups when assistant structure changes in place", () => {
  const created_at = "2026-01-01T00:00:00.000Z";
  const assistant: BotMessage = {
    id: "assistant-1",
    type: "assistant",
    content: [{ type: "thinking", thinking: "working" }],
    created_at,
  };
  const tool: ToolMessage = {
    id: "tool-1",
    type: "tool",
    tool_call_id: "call-1",
    tool_name: "read",
    status: "completed",
    arguments: "{}",
    result: [],
    created_at,
  };
  const boundary: UserMessage = {
    id: "user-2",
    type: "user",
    content: [{ type: "text", text: "next" }],
    created_at,
  };
  const stable: Message[] = [assistant, tool, boundary];
  const cache = new DisplayItemProjection();
  const before = cache.update("session-a", stable, 0, [], false);

  assistant.content.push({ type: "text", text: "done" });
  const after = cache.update("session-a", stable, 1, [], false);

  expect(after.stableItems).not.toBe(before.stableItems);
  const projected = [...after.stableItems, ...after.dynamicItems];
  expect(projected.map((item) => item.type)).toEqual(
    buildDisplayItems(stable, false).map((item) => item.type),
  );
  expect(projected).toHaveLength(3);
});

test("preserves cached items for non-structural in-place mutations", () => {
  const created_at = "2026-01-01T00:00:00.000Z";
  const tool: ToolMessage = {
    id: "tool-1",
    type: "tool",
    tool_call_id: "call-1",
    tool_name: "read",
    status: "running",
    arguments: "{}",
    result: [],
    created_at,
  };
  const boundary: UserMessage = {
    id: "user-2",
    type: "user",
    content: [{ type: "text", text: "next" }],
    created_at,
  };
  const stable: Message[] = [tool, boundary];
  const cache = new DisplayItemProjection();
  const before = cache.update("session-a", stable, 0, [], false);

  tool.status = "completed";
  tool.result = [{ type: "text", text: "done" }];
  tool.elapsed_ms = 42;
  const after = cache.update("session-a", stable, 0, [], false);

  expect(after.stableItems).toBe(before.stableItems);
  expect(after.stableItems[0]).toBe(before.stableItems[0]);
  expect(tool.result).toEqual([{ type: "text", text: "done" }]);
});

test("seals a closed committed tail away from later streaming updates", () => {
  const created_at = "2026-01-01T00:00:00.000Z";
  const messages: Message[] = [
    {
      id: "user-1",
      type: "user",
      content: [{ type: "text", text: "start" }],
      created_at,
    },
    {
      id: "assistant-1",
      type: "assistant",
      content: [{ type: "text", text: "tail" }],
      created_at,
    },
  ];
  const cache = new DisplayItemProjection();

  const first = cache.update("session-a", messages, 0, [], false);
  expect(first.stableItems).toHaveLength(2);
  expect(first.dynamicItems).toEqual([]);

  const sealedItems = first.stableItems;
  const streaming: Message = {
    id: "assistant-stream",
    type: "assistant",
    content: [{ type: "text", text: "working" }],
    created_at,
  };
  const second = cache.update("session-a", messages, 0, [streaming], true);
  expect(second.stableItems).toBe(sealedItems);
  expect(second.dynamicItems).toMatchObject([
    { type: "message", message: streaming, isStreaming: true },
  ]);
});

test("keeps the same live activity group while interstitial text streams", () => {
  const created_at = "2026-01-01T00:00:00.000Z";
  const message: BotMessage = {
    id: "assistant-1",
    type: "assistant",
    content: [{ type: "thinking", thinking: "working" }],
    created_at,
  };

  // Thinking streams first: the tail group is the live activity.
  let items = buildDisplayItems([message], true);
  expect(liveActivityIndex(items)).toBe(0);

  // Interstitial text arrives: the group must stay live — the text may still
  // be followed by tool calls, so it cannot be judged final mid-stream.
  message.content.push({ type: "text", text: "Let me check a file" });
  items = buildDisplayItems([message], true);
  expect(liveActivityIndex(items)).toBe(0);

  // Tool calls follow on the same message: same live group, no flap.
  message.tool_calls = [{ id: "call-1", name: "read", arguments: "{}" }];
  items = buildDisplayItems([message], true);
  expect(liveActivityIndex(items)).toBe(0);

  // A trailing standalone message (the final answer) takes over the tail.
  const answer: BotMessage = {
    id: "assistant-2",
    type: "assistant",
    content: [{ type: "text", text: "done" }],
    created_at,
  };
  items = buildDisplayItems([message, answer], true);
  expect(liveActivityIndex(items)).toBe(-1);
});

test("uses occurrence keys without coupling identity to absolute position", () => {
  const created_at = "2026-01-01T00:00:00.000Z";
  const assistant = (id: string, text: string): BotMessage => ({
    id,
    type: "assistant",
    content: [{ type: "text", text }],
    created_at,
  });
  const duplicateItems = buildDisplayItems(
    [assistant("dup", "one"), assistant("dup", "two")],
    false,
  );
  const before = keyDisplayItems(duplicateItems).map((item) => item.key);
  const prefix = buildDisplayItems(
    [
      {
        id: "user-1",
        type: "user",
        content: [{ type: "text", text: "prefix" }],
        created_at,
      },
      assistant("dup", "one"),
      assistant("dup", "two"),
    ],
    false,
  );
  const after = keyDisplayItems(prefix).map((item) => item.key);

  expect(before).toEqual([
    "message:assistant:dup:0",
    "message:assistant:dup:1",
  ]);
  expect(after.slice(1)).toEqual(before);
});
