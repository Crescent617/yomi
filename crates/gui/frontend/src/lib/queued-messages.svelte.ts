import * as api from "./api";
import type { TaggedContentBlock } from "./types";

/** A message waiting for its session to become idle. */
export interface QueuedMessage {
  text: string;
  blocks?: TaggedContentBlock[];
}

/**
 * Per-session queued messages — at most one per session. Flushed by the
 * global notification listener when a session transitions to idle, so
 * delivery works regardless of which session is currently active.
 */
export const queuedMessages = $state<Record<string, QueuedMessage>>({});

/** Queue a message; returns false when the session already has one queued. */
export function queueMessage(
  sessionId: string,
  message: QueuedMessage,
): boolean {
  if (queuedMessages[sessionId]) return false;
  queuedMessages[sessionId] = message;
  return true;
}

export function clearQueuedMessage(sessionId: string): void {
  delete queuedMessages[sessionId];
}

/**
 * Send the queued message (if any) for a session that just became idle.
 * On failure the message is restored so it can be retried or cancelled,
 * unless the user queued a new message while the send was in flight.
 * Returns false only when a send was attempted and failed.
 */
export async function flushQueuedMessage(sessionId: string): Promise<boolean> {
  const queued = queuedMessages[sessionId];
  if (!queued) return true;
  delete queuedMessages[sessionId];
  try {
    if (queued.blocks && queued.blocks.length > 0) {
      await api.sendMessageBlocks(sessionId, queued.blocks);
    } else {
      await api.sendMessage(sessionId, queued.text);
    }
    return true;
  } catch (e) {
    console.error(
      "Failed to send queued message:",
      e instanceof Error ? e.message : e,
    );
    // Restore for retry/cancel, but never clobber a message the user
    // queued while this send was in flight.
    queuedMessages[sessionId] ??= queued;
    return false;
  }
}
