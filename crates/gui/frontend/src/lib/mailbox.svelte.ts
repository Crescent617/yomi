import * as api from "./api";
import type { MailboxItem, MailboxSnapshot } from "./api";
import type { TaggedContentBlock } from "./types";

/**
 * Mailbox-backed pending state（取代旧的本地单槽队列）。
 *
 * - `mailboxBySession`：当前打开的会话的 pending 快照（PendingBar 渲染源）。
 * - `pendingCounts`：事件驱动的双队列计数（会话列表徽标）。
 * 刷新时机：会话打开/切换时拉一次快照；此后靠 `mailbox_changed`
 * 通知（入队/消费/撤回/清空都会发）驱动计数与快照刷新。
 */

export const mailboxBySession = $state<Record<string, MailboxSnapshot>>({});
export const pendingCounts = $state<
  Record<string, { steer: number; queued: number }>
>({});

export async function refreshMailbox(sessionId: string): Promise<void> {
  try {
    mailboxBySession[sessionId] = await api.mailboxSnapshot(sessionId);
  } catch (e) {
    console.error(
      "Failed to load mailbox:",
      e instanceof Error ? e.message : e,
    );
  }
}

/** 全局通知钩子（state.svelte.ts 的 kernel:noti 监听器调用）。 */
export function onMailboxChanged(
  sessionId: string,
  steer: number,
  queued: number,
  activeSessionId: string | null,
): void {
  pendingCounts[sessionId] = { steer, queued };
  if (activeSessionId === sessionId) void refreshMailbox(sessionId);
}

/** 会话列表徽标计数（steer + queue）。 */
export function pendingOf(sessionId: string): number {
  const c = pendingCounts[sessionId];
  return c ? c.steer + c.queued : 0;
}

/** 本地手势作用的对象：queue 队首（FIFO 最旧的排队消息）。 */
export function queueHead(sessionId: string): MailboxItem | undefined {
  return mailboxBySession[sessionId]?.queue[0];
}

/** 排队（单槽语义：已有排队消息时拒绝）。 */
export async function enqueue(
  sessionId: string,
  text: string,
  blocks?: TaggedContentBlock[],
): Promise<boolean> {
  if (queueHead(sessionId)) return false;
  if (blocks && blocks.length > 0) {
    await api.sendMessageBlocks(sessionId, blocks);
  } else {
    await api.sendMessage(sessionId, text);
  }
  void refreshMailbox(sessionId);
  return true;
}

/** 把 queue 队首提升为 steer 注入当前 run。 */
export async function steerQueueHead(sessionId: string): Promise<boolean> {
  const head = queueHead(sessionId);
  if (!head) return false;
  const moved = await api.steerMailboxItem(sessionId, head.id);
  void refreshMailbox(sessionId);
  return moved;
}

/** 撤回一条 pending。 */
export async function retractMailboxItem(
  sessionId: string,
  itemId: string,
): Promise<void> {
  await api.removeMailboxItem(sessionId, itemId);
  void refreshMailbox(sessionId);
}
