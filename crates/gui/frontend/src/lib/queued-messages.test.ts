import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("./api", () => ({
  sendMessage: vi.fn(),
  sendMessageBlocks: vi.fn(),
}));

import * as api from "./api";
import {
  clearQueuedMessage,
  flushQueuedMessage,
  queueMessage,
  queuedMessages,
} from "./queued-messages.svelte";

beforeEach(() => {
  for (const key of Object.keys(queuedMessages)) delete queuedMessages[key];
  vi.mocked(api.sendMessage).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.sendMessageBlocks).mockReset().mockResolvedValue(undefined);
});

describe("queued message store", () => {
  test("queues at most one message per session", () => {
    expect(queueMessage("s1", { text: "first" })).toBe(true);
    expect(queueMessage("s1", { text: "second" })).toBe(false);
    expect(queuedMessages["s1"]).toEqual({ text: "first" });

    // other sessions are independent
    expect(queueMessage("s2", { text: "other" })).toBe(true);
    expect(queuedMessages["s2"]).toEqual({ text: "other" });
  });

  test("clear removes the queued message", () => {
    queueMessage("s1", { text: "first" });
    clearQueuedMessage("s1");
    expect(queuedMessages["s1"]).toBeUndefined();
    expect(queueMessage("s1", { text: "again" })).toBe(true);
  });

  test("flush sends text messages and empties the queue", async () => {
    queueMessage("s1", { text: "hello" });

    await expect(flushQueuedMessage("s1")).resolves.toBe(true);

    expect(api.sendMessage).toHaveBeenCalledWith("s1", "hello");
    expect(api.sendMessageBlocks).not.toHaveBeenCalled();
    expect(queuedMessages["s1"]).toBeUndefined();
  });

  test("flush prefers content blocks when present", async () => {
    const blocks = [{ type: "text", text: "with image" }];
    queueMessage("s1", { text: "with image", blocks });

    await expect(flushQueuedMessage("s1")).resolves.toBe(true);

    expect(api.sendMessageBlocks).toHaveBeenCalledWith("s1", blocks);
    expect(api.sendMessage).not.toHaveBeenCalled();
  });

  test("flush is a no-op when nothing is queued", async () => {
    await expect(flushQueuedMessage("s1")).resolves.toBe(true);
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(api.sendMessageBlocks).not.toHaveBeenCalled();
  });

  test("flush restores the message when the send fails", async () => {
    vi.mocked(api.sendMessage).mockRejectedValue(new Error("connection lost"));
    queueMessage("s1", { text: "retry me" });

    await expect(flushQueuedMessage("s1")).resolves.toBe(false);

    expect(queuedMessages["s1"]).toEqual({ text: "retry me" });
  });

  test("a failed flush never clobbers a message queued mid-flight", async () => {
    let rejectSend!: (e: Error) => void;
    vi.mocked(api.sendMessage).mockImplementation(
      () => new Promise<void>((_, reject) => (rejectSend = reject)),
    );
    queueMessage("s1", { text: "old" });

    const flush = flushQueuedMessage("s1");
    // user queues a new message while the first send is still in flight
    expect(queueMessage("s1", { text: "new" })).toBe(true);
    rejectSend(new Error("connection lost"));

    await expect(flush).resolves.toBe(false);
    expect(queuedMessages["s1"]).toEqual({ text: "new" });
  });
});
