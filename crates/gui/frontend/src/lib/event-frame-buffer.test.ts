import { afterEach, beforeEach, describe, expect, test } from "vitest";
import {
  EventFrameBuffer,
  type KernelEventEnvelope,
} from "./event-frame-buffer";

const originalRequestAnimationFrame = globalThis.requestAnimationFrame;
const originalCancelAnimationFrame = globalThis.cancelAnimationFrame;
let nextFrameId = 0;
let frameTimers = new Map<number, ReturnType<typeof setTimeout>>();

beforeEach(() => {
  globalThis.requestAnimationFrame = (callback) => {
    const id = ++nextFrameId;
    frameTimers.set(
      id,
      setTimeout(() => {
        frameTimers.delete(id);
        callback(performance.now());
      }, 0),
    );
    return id;
  };
  globalThis.cancelAnimationFrame = (id) => {
    const timer = frameTimers.get(id);
    if (timer !== undefined) clearTimeout(timer);
  };
});

afterEach(() => {
  for (const timer of frameTimers.values()) clearTimeout(timer);
  frameTimers = new Map<number, ReturnType<typeof setTimeout>>();
  globalThis.requestAnimationFrame = originalRequestAnimationFrame;
  globalThis.cancelAnimationFrame = originalCancelAnimationFrame;
});

function textDelta(
  session_id: string,
  message_id: string,
  event_id: string,
  text = "x",
): KernelEventEnvelope {
  return {
    session_id,
    event_id,
    event: { model: { chunk: { message_id, content: { text } } } },
  };
}

describe("kernel event frame buffer", () => {
  test("merges only consecutive matching text or thinking deltas", () => {
    const dispatched: KernelEventEnvelope[] = [];
    const buffer = new EventFrameBuffer((event) => dispatched.push(event));

    buffer.enqueue(textDelta("session-a", "message-a", "text-1", "hel"));
    buffer.enqueue(textDelta("session-a", "message-a", "text-2", "lo"));
    buffer.enqueue({
      session_id: "session-a",
      event_id: "thinking-1",
      event: {
        model: {
          chunk: {
            message_id: "message-a",
            content: { thinking: { thinking: "hmm" } },
          },
        },
      },
    });
    buffer.enqueue({
      session_id: "session-a",
      event_id: "thinking-2",
      event: {
        model: {
          chunk: {
            message_id: "message-a",
            content: { thinking: { thinking: "..." } },
          },
        },
      },
    });
    buffer.flush();

    expect(dispatched).toEqual([
      textDelta("session-a", "message-a", "text-2", "hello"),
      {
        session_id: "session-a",
        event_id: "thinking-2",
        event: {
          model: {
            chunk: {
              message_id: "message-a",
              content: { thinking: { thinking: "hmm..." } },
            },
          },
        },
      },
    ]);
  });

  test("key changes split merge lanes without flushing; non-deltas stay synchronous barriers", () => {
    const ids: Array<string | undefined> = [];
    const buffer = new EventFrameBuffer((event) => ids.push(event.event_id));

    buffer.enqueue(textDelta("session-a", "message-a", "first"));
    buffer.enqueue(textDelta("session-b", "message-a", "second"));
    const afterSessionChange = [...ids];
    buffer.enqueue({
      session_id: "session-b",
      event_id: "barrier",
      event: { agent: { state_changed: { state: "streaming" } } },
    });
    const afterBarrier = [...ids];
    buffer.enqueue(textDelta("session-b", "message-b", "third"));
    buffer.enqueue(textDelta("session-b", "message-c", "fourth"));
    const afterMessageChange = [...ids];
    buffer.enqueue({
      session_id: "session-b",
      event_id: "tool-delta",
      event: {
        model: {
          tool_call_delta: {
            message_id: "message-c",
            tool_id: "tool-a",
            tool_name: "read",
            arguments_delta: "{}",
          },
        },
      },
    });

    expect({
      afterSessionChange,
      afterBarrier,
      afterMessageChange,
      final: ids,
    }).toEqual({
      // A session switch no longer flushes: "first" waits for the next
      // frame tick (or the barrier below) instead of jumping the queue.
      afterSessionChange: [],
      afterBarrier: ["first", "second", "barrier"],
      afterMessageChange: ["first", "second", "barrier"],
      final: ["first", "second", "barrier", "third", "fourth", "tool-delta"],
    });
  });

  test("interleaved sessions drain in arrival order, throttled (no per-event flush)", () => {
    const ids: Array<string | undefined> = [];
    const buffer = new EventFrameBuffer((event) => ids.push(event.event_id));

    buffer.enqueue(textDelta("session-a", "message-a", "a1"));
    buffer.enqueue(textDelta("session-b", "message-a", "b1"));
    buffer.enqueue(textDelta("session-a", "message-a", "a2"));
    buffer.enqueue(textDelta("session-b", "message-a", "b2"));
    // Alternating keys must not force one dispatch per event.
    expect(ids).toEqual([]);

    buffer.flush();
    // Interleaved deltas don't merge across lanes, but arrival order is
    // preserved and a single flush drains everything.
    expect(ids).toEqual(["a1", "b1", "a2", "b2"]);
  });

  test("clones buffered input and retains the latest event_id", () => {
    const dispatched: KernelEventEnvelope[] = [];
    const buffer = new EventFrameBuffer((event) => dispatched.push(event));
    const first = textDelta("session-a", "message-a", "old", "a");

    buffer.enqueue(first);
    buffer.enqueue(textDelta("session-a", "message-a", "new", "b"));
    buffer.flush();

    expect(first).toEqual(textDelta("session-a", "message-a", "old", "a"));
    expect(dispatched).toEqual([
      textDelta("session-a", "message-a", "new", "ab"),
    ]);
  });

  test("flushes on a frame and cancels the timeout fallback", async () => {
    let count = 0;
    const buffer = new EventFrameBuffer(() => count++);
    buffer.enqueue(textDelta("session-a", "message-a", "frame"));

    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(count).toBe(1);
  });

  test("flushes immediately at item and character capacity", () => {
    const itemDispatches: string[] = [];
    const itemBuffer = new EventFrameBuffer((event) => {
      const model = event.event as {
        model: { chunk: { content: { text: string } } };
      };
      itemDispatches.push(model.model.chunk.content.text);
    });
    for (let i = 0; i < 1024; i++) {
      itemBuffer.enqueue(textDelta("session-a", "message-a", String(i)));
    }

    const charDispatches: number[] = [];
    const charBuffer = new EventFrameBuffer((event) => {
      const model = event.event as {
        model: { chunk: { content: { text: string } } };
      };
      charDispatches.push(model.model.chunk.content.text.length);
    });
    charBuffer.enqueue(
      textDelta("session-b", "message-b", "large", "x".repeat(256 * 1024)),
    );

    expect({
      item_count: itemDispatches.length,
      item_chars: itemDispatches[0]?.length,
      char_dispatches: charDispatches,
    }).toEqual({
      item_count: 1,
      item_chars: 1024,
      char_dispatches: [256 * 1024],
    });
  });

  test("throttles consecutive frame flushes to the minimum interval", async () => {
    let count = 0;
    const buffer = new EventFrameBuffer(() => count++);

    buffer.enqueue(textDelta("session-a", "message-a", "first"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    // First flush lands on the next frame — no throttle on an idle buffer.
    expect(count).toBe(1);

    buffer.enqueue(textDelta("session-a", "message-a", "second"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    // Within the 66ms window: held back.
    expect(count).toBe(1);
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(count).toBe(2);
  });

  test("an empty flush does not restart the throttle clock", async () => {
    let count = 0;
    const buffer = new EventFrameBuffer(() => count++);

    buffer.enqueue(textDelta("session-a", "message-a", "first"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(count).toBe(1);

    // Wait out the 66ms window, then flush an empty queue — this is what a
    // barrier event arriving between text bursts does. It dispatches
    // nothing, so it must not move the clock.
    await new Promise((resolve) => setTimeout(resolve, 100));
    buffer.flush();

    buffer.enqueue(textDelta("session-a", "message-a", "second"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    // The buffer has been idle past the interval: lands on the next frame,
    // not held back by the empty flush.
    expect(count).toBe(2);
  });

  test("flushes pending events on disposal", () => {
    const ids: Array<string | undefined> = [];
    const buffer = new EventFrameBuffer((event) => ids.push(event.event_id));

    buffer.enqueue(textDelta("session-c", "message-a", "dispose"));
    buffer.dispose();
    buffer.enqueue(
      textDelta("session-c", "message-a", "ignored-after-dispose"),
    );

    expect(ids).toEqual(["dispose"]);
  });
});
