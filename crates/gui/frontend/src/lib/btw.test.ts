import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { btwState, handleBtwEvent, closeBtwCard } from "./btw.svelte";

function seedCard(status: "pending" | "streaming" = "pending") {
  btwState.card = {
    sessionId: "sess_1",
    requestId: "btw_1",
    question: "刚才那个变量叫什么？",
    text: "",
    status,
    toolUseFallback: false,
  };
}

describe("btw card state", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    btwState.card = null;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("throttles deltas into a 66ms flush, then settles on stop", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      delta: { request_id: "btw_1", text: "hel" },
    });
    handleBtwEvent("sess_1", {
      delta: { request_id: "btw_1", text: "lo" },
    });
    // 聚合窗口内不落字；窗口到点一次性追加（O(delta) 流式规则）。
    expect(btwState.card!.text).toBe("");
    vi.advanceTimersByTime(66);
    expect(btwState.card!.text).toBe("hello");
    expect(btwState.card!.status).toBe("streaming");

    handleBtwEvent("sess_1", {
      delta: { request_id: "btw_1", text: "!" },
    });
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: "stop" },
    });
    // done 同步收干未 flush 的尾巴。
    expect(btwState.card!.text).toBe("hello!");
    expect(btwState.card!.status).toBe("done");
  });

  it("flags the tool-use fallback only when no text arrived", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: "tool_use" },
    });
    expect(btwState.card!.toolUseFallback).toBe(true);

    seedCard("streaming");
    btwState.card!.text = "这需要正式提问";
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: "tool_use" },
    });
    expect(btwState.card!.toolUseFallback).toBe(false);
  });

  it("destroys the card on replaced and cancelled", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: "replaced" },
    });
    expect(btwState.card).toBeNull();

    seedCard();
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: "cancelled" },
    });
    expect(btwState.card).toBeNull();
  });

  it("surfaces errors with the message", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      done: { request_id: "btw_1", reason: { error: "rate limit" } },
    });
    expect(btwState.card!.status).toBe("error");
    expect(btwState.card!.error).toBe("rate limit");
  });

  it("ignores events for other request ids and other sessions", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      delta: { request_id: "btw_other", text: "nope" },
    });
    handleBtwEvent("sess_other", {
      delta: { request_id: "btw_1", text: "nope" },
    });
    vi.advanceTimersByTime(66);
    expect(btwState.card!.text).toBe("");
    expect(btwState.card!.status).toBe("pending");
  });

  it("closeBtwCard clears state and pending flushes", () => {
    seedCard();
    handleBtwEvent("sess_1", {
      delta: { request_id: "btw_1", text: "x" },
    });
    closeBtwCard();
    vi.advanceTimersByTime(66);
    expect(btwState.card).toBeNull();
  });
});
