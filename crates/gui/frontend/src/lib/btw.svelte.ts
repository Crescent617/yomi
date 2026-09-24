// `/btw` 旁问浮卡状态：每会话一张卡，关闭即销毁（与内核"不落盘"同语义）。
// daemon 侧同时只跑一条旁问——新提问替换旧流，旧流以 `replaced` 收尾。

import { btw as btwCmd } from "./api";
import type { BtwEvent } from "./state.svelte";

export type BtwStatus = "pending" | "streaming" | "done" | "error";

export interface BtwCardState {
  sessionId: string;
  requestId: string;
  question: string;
  text: string;
  status: BtwStatus;
  error?: string;
  /** 模型只发 tool_use 没给文本时，显示"这需要正式提问"兜底。 */
  toolUseFallback: boolean;
}

export const BTW_TOOL_FALLBACK =
  "这需要正式提问——旁问只能基于当前会话里已有的信息回答。";

export const btwState = $state<{ card: BtwCardState | null }>({ card: null });

// 与 EventFrameBuffer 同档：delta 聚合到 ~15fps，每次 flush 的成本是
// O(新字节) 而不是 O(全文)（DESIGN.md 的流式规则）。
const FLUSH_MS = 66;
let pendingText = "";
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function clearFlush() {
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  pendingText = "";
}

function flushDelta() {
  flushTimer = null;
  if (!btwState.card || !pendingText) {
    pendingText = "";
    return;
  }
  btwState.card.text += pendingText;
  pendingText = "";
}

export async function askBtw(
  sessionId: string,
  question: string,
): Promise<void> {
  // request_id 客户端铸造、建卡先于 invoke：webview 里 invoke 应答与
  // 事件回调的相对顺序不做假设，任何到达顺序下事件都能对上卡。
  const requestId = `btw_${crypto.randomUUID()}`;
  clearFlush();
  btwState.card = {
    sessionId,
    requestId,
    question,
    text: "",
    status: "pending",
    toolUseFallback: false,
  };
  try {
    await btwCmd(sessionId, question, requestId);
  } catch (e) {
    // 发问失败：daemon 从未见过这条 request_id，销毁刚建的卡再上抛
    //（由调用方出通知）。
    if (btwState.card?.requestId === requestId) btwState.card = null;
    throw e;
  }
}

export function closeBtwCard(): void {
  clearFlush();
  btwState.card = null;
}

export function handleBtwEvent(sessionId: string, ev: BtwEvent): void {
  const card = btwState.card;
  if (!card || card.sessionId !== sessionId) return;

  // 终态之后的事件一律忽略：abort 与任务自然收尾之间存在亚毫秒竞态
  //（Done→Delta / Done→Done），卡片状态只许前进不许回退。
  if (card.status === "done" || card.status === "error") return;

  if (ev.start) {
    // 卡在 askBtw 里已建成 pending；Start 只是生命周期起点确认，无操作。
    return;
  }
  if (ev.delta) {
    if (ev.delta.request_id !== card.requestId) return;
    card.status = "streaming";
    pendingText += ev.delta.text;
    if (flushTimer === null) {
      flushTimer = setTimeout(flushDelta, FLUSH_MS);
    }
    return;
  }
  if (ev.done) {
    if (ev.done.request_id !== card.requestId) return;
    flushDelta();
    clearFlush();
    const reason = ev.done.reason;
    if (reason === "stop") {
      card.status = "done";
    } else if (reason === "tool_use") {
      card.toolUseFallback = card.text.trim().length === 0;
      card.status = "done";
    } else if (reason === "replaced" || reason === "cancelled") {
      // 被更新的旁问替换 / 会话取消：关闭即销毁，不留卡片。
      btwState.card = null;
    } else {
      card.error = reason.error;
      card.status = "error";
    }
  }
}
