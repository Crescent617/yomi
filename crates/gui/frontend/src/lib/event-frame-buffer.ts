import type { KernelEvent } from "./state.svelte";

const MAX_QUEUED_ITEMS = 1024;
const MAX_QUEUED_CHARS = 256 * 1024;
const TIMEOUT_MS = 100;
/**
 * Minimum interval between streamed-text dispatches. Rendering markdown at
 * the display refresh rate costs a full reactive pass per frame; ~15fps is
 * indistinguishable for text while cutting the per-second render cost 4x.
 */
const MIN_FLUSH_INTERVAL_MS = 66;

export interface KernelEventEnvelope {
  session_id: string;
  event_id?: string;
  event: unknown;
}

type DeltaKind = "text" | "thinking";

type BufferedEvent = {
  envelope: KernelEventEnvelope;
  kind: DeltaKind;
  message_id: string;
};

type Dispatch = (envelope: KernelEventEnvelope) => void;

function getDelta(envelope: KernelEventEnvelope): {
  kind: DeltaKind;
  message_id: string;
  value: string;
} | null {
  const event = envelope.event as KernelEvent;
  if (
    event === null ||
    typeof event !== "object" ||
    !("model" in event) ||
    !event.model.chunk
  ) {
    return null;
  }

  const { message_id, content } = event.model.chunk;
  if (typeof content.text === "string") {
    return { kind: "text", message_id, value: content.text };
  }
  if (typeof content.thinking?.thinking === "string") {
    return {
      kind: "thinking",
      message_id,
      value: content.thinking.thinking,
    };
  }
  return null;
}

function cloneEnvelope(envelope: KernelEventEnvelope): KernelEventEnvelope {
  return structuredClone(envelope);
}

function appendDelta(
  buffered: BufferedEvent,
  value: string,
  event_id?: string,
) {
  const event = buffered.envelope.event as KernelEvent;
  const content = "model" in event ? event.model.chunk?.content : undefined;
  if (!content) return;

  if (buffered.kind === "text") {
    content.text = (content.text ?? "") + value;
  } else if (content.thinking) {
    content.thinking.thinking = (content.thinking.thinking ?? "") + value;
  }
  if (event_id !== undefined) {
    buffered.envelope.event_id = event_id;
  }
}

export class EventFrameBuffer {
  private queue: BufferedEvent[] = [];
  private queuedItems = 0;
  private queuedChars = 0;
  private frameId: number | null = null;
  private timeoutId: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  private lastFlushAt = Number.NEGATIVE_INFINITY;

  constructor(
    private readonly dispatch: Dispatch,
    private readonly minFlushIntervalMs = MIN_FLUSH_INTERVAL_MS,
  ) {}

  enqueue(envelope: KernelEventEnvelope) {
    if (this.disposed) return;

    const delta = getDelta(envelope);
    if (!delta) {
      this.flush();
      this.dispatch(envelope);
      return;
    }

    const last = this.queue[this.queue.length - 1];
    // Merge only into a same-lane tail (same session + message + kind);
    // otherwise append a new entry. A key change deliberately does NOT
    // flush: the queue drains in arrival order on the next frame tick, so
    // interleaved streams from concurrent sessions stay throttled at the
    // frame interval instead of forcing one dispatch per event.
    if (
      last &&
      last.envelope.session_id === envelope.session_id &&
      last.message_id === delta.message_id &&
      last.kind === delta.kind
    ) {
      appendDelta(last, delta.value, envelope.event_id);
    } else {
      this.queue.push({
        envelope: cloneEnvelope(envelope),
        kind: delta.kind,
        message_id: delta.message_id,
      });
    }

    this.queuedItems += 1;
    this.queuedChars += delta.value.length;
    if (
      this.queuedItems >= MAX_QUEUED_ITEMS ||
      this.queuedChars >= MAX_QUEUED_CHARS
    ) {
      this.flush();
    } else {
      this.scheduleFlush();
    }
  }

  flush() {
    this.cancelScheduledFlush();
    const queue = this.queue;
    // An empty flush dispatches nothing, so it must not restart the
    // throttle clock — otherwise text arriving just after a barrier event
    // is held back for a full extra interval.
    if (queue.length === 0) return;
    this.lastFlushAt = performance.now();
    this.queue = [];
    this.queuedItems = 0;
    this.queuedChars = 0;
    for (const item of queue) {
      this.dispatch(item.envelope);
    }
  }

  dispose() {
    if (this.disposed) return;
    this.flush();
    this.disposed = true;
  }

  private scheduleFlush() {
    if (this.frameId !== null || this.timeoutId !== null) return;
    this.frameId = requestAnimationFrame(() => {
      this.frameId = null;
      const elapsed = performance.now() - this.lastFlushAt;
      if (elapsed >= this.minFlushIntervalMs) {
        this.flush();
        return;
      }
      // Flushed too recently: land the pending text when the interval
      // completes instead of paying a render for every display frame.
      if (this.timeoutId !== null) clearTimeout(this.timeoutId);
      this.timeoutId = setTimeout(
        () => this.flush(),
        this.minFlushIntervalMs - elapsed,
      );
    });
    this.timeoutId = setTimeout(() => this.flush(), TIMEOUT_MS);
  }

  private cancelScheduledFlush() {
    if (this.frameId !== null) {
      cancelAnimationFrame(this.frameId);
      this.frameId = null;
    }
    if (this.timeoutId !== null) {
      clearTimeout(this.timeoutId);
      this.timeoutId = null;
    }
  }
}
