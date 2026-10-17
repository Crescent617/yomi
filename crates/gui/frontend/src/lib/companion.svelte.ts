// 伙伴悬浮窗（companion）前端控制器：只在 pet 窗口的 JS 上下文里运行。
//
// 绑定语义：pinned（设置里固定）> override（点角标/通知一次性跳转，关面板即清）
// > follow（主窗口活跃 session，随 PetSnapshot.active_session_id 推送）。
// 绑定的 session 走本窗口自己的一份 session store（每个 webview 上下文独立），
// handleEvent/MessageList/PermissionBar 等共享模块因此零改动复用。

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as api from "./api";
import type {
  CompanionOpenChanged,
  CompanionPlacement,
  CompanionState,
  PetSnapshot,
} from "./api";
import { getSession, startNotificationListener } from "./state.svelte";
import { loadSessionData, setActiveSession } from "./session";
import { handleEvent } from "./events";
import {
  EventFrameBuffer,
  type KernelEventEnvelope,
} from "./event-frame-buffer";

export const companionState = $state({
  dto: null as CompanionState | null,
  open: false,
  /** 收起动画播放中：面板还在渲染（播 out 动画），播完才调 Rust 缩窗。 */
  closing: false,
  placement: "above" as CompanionPlacement,
  snapshot: null as PetSnapshot | null,
  override_session_id: null as string | null,
  sprite_scale: null as number | null,
  sprite_x: null as number | null,
});

/** 面板收起动画时长（与 pet 页 CSS 保持一致）；reduced-motion 时为 0。 */
export const PANEL_CLOSE_MS = 150;

/** 纯函数，绑定优先级见文件头注释（vitest 覆盖）。 */
export function resolveBoundSessionId(input: {
  pinned_session_id: string | null;
  override_session_id: string | null;
  active_session_id: string | null;
}): string | null {
  return (
    input.pinned_session_id ??
    input.override_session_id ??
    input.active_session_id ??
    null
  );
}

export function boundSessionId(): string | null {
  return resolveBoundSessionId({
    pinned_session_id: companionState.dto?.pinned_session_id ?? null,
    override_session_id: companionState.override_session_id,
    active_session_id: companionState.snapshot?.active_session_id ?? null,
  });
}

/** 当前绑定 session 在本窗口 store 里的状态（未水合时为 undefined）。 */
export function boundSession() {
  const id = boundSessionId();
  return id ? getSession(id) : undefined;
}

export async function toggleCompanionOpen(): Promise<void> {
  if (companionState.closing) return;
  if (companionState.open) {
    await closeCompanion();
    return;
  }
  // 从有待处理请求的 alert 状态进入时，直接落到那个 session（pin 优先，
  // 尊重用户的固定选择）。
  const request = companionState.snapshot?.request;
  if (request && !companionState.dto?.pinned_session_id) {
    companionState.override_session_id = request.session_id;
  }
  await api.setCompanionOpen(true);
}

export async function closeCompanion(): Promise<void> {
  if (!companionState.open || companionState.closing) return;
  // 先播收起动画，再让 Rust 缩窗——窗口瞬时缩小会切断动画。
  const reduce_motion =
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  companionState.closing = true;
  if (!reduce_motion) {
    await new Promise((resolve) => setTimeout(resolve, PANEL_CLOSE_MS));
  }
  companionState.closing = false;
  if (companionState.open) await api.setCompanionOpen(false);
}

export async function togglePinnedSession(): Promise<void> {
  const dto = companionState.dto;
  if (!dto) return;
  if (dto.pinned_session_id) {
    await api.setCompanionPinnedSession(null);
    return;
  }
  const id = boundSessionId();
  if (id) await api.setCompanionPinnedSession(id);
}

// ── 初始化与绑定同步 ─────────────────────────────────────────────────────

let started = false;
let bound_applied: string | null = null;

async function syncBoundSession(id: string | null): Promise<void> {
  if (id === bound_applied) return;
  bound_applied = id;
  if (!id) {
    setActiveSession(null);
    return;
  }
  const generation = id;
  await loadSessionData(id).catch((error) => {
    console.error("Failed to load companion session:", error);
  });
  // 水合期间绑定又变了，放弃这次应用。
  if (bound_applied !== generation) return;
  setActiveSession(id);
}

/**
 * pet 窗口启动时调用一次；返回清理函数。事件源：
 * - pet:state / companion:state / companion:open_changed（Rust 推送）
 * - kernel:event（广播，EventFrameBuffer 同档节流后喂给共享 fold）
 * - kernel:noti（passive：只更新本地 phase/title，不产生通知类副作用）
 */
export async function initCompanion(): Promise<() => void> {
  if (started) return () => {};
  started = true;

  companionState.dto = await api.getCompanionState().catch((error) => {
    console.error("Failed to load companion state:", error);
    return null;
  });
  companionState.snapshot = await api.getPetState().catch((error) => {
    console.error("Failed to load pet snapshot:", error);
    return null;
  });
  companionState.open = companionState.dto?.open ?? false;

  const unlisteners: UnlistenFn[] = [];
  unlisteners.push(
    await listen<PetSnapshot>("pet:state", (event) => {
      const previous = companionState.snapshot;
      if (previous && event.payload.revision < previous.revision) return;
      companionState.snapshot = event.payload;
    }),
  );
  unlisteners.push(
    await listen<CompanionState>("companion:state", (event) => {
      companionState.dto = event.payload;
    }),
  );
  unlisteners.push(
    await listen<CompanionOpenChanged>("companion:open_changed", (event) => {
      companionState.open = event.payload.open;
      companionState.sprite_scale = event.payload.sprite_scale;
      companionState.sprite_x = event.payload.sprite_x;
      if (event.payload.placement) {
        companionState.placement = event.payload.placement;
      }
      if (!event.payload.open) {
        companionState.override_session_id = null;
      }
    }),
  );
  // 热键意图由前端驱动实际开合（收起动画播完才缩窗）。
  unlisteners.push(
    await listen("companion:toggle_requested", () => {
      void toggleCompanionOpen();
    }),
  );

  const eventFrameBuffer = new EventFrameBuffer(
    ({ session_id, event_id, event }) => {
      if (getSession(session_id)) {
        handleEvent(session_id, event_id, event);
      }
    },
  );
  unlisteners.push(
    await listen<KernelEventEnvelope>("kernel:event", (event) => {
      eventFrameBuffer.enqueue(event.payload);
    }),
  );
  unlisteners.push(await startNotificationListener({ passive: true }));

  // 绑定同步：面板展开时才持有订阅（收起时不占 session 泵的 refcount）。
  const dispose_binding = $effect.root(() => {
    $effect(() => {
      const id = companionState.open ? boundSessionId() : null;
      void syncBoundSession(id);
    });
  });

  return () => {
    started = false;
    bound_applied = null;
    dispose_binding();
    eventFrameBuffer.dispose();
    for (const unlisten of unlisteners) unlisten();
  };
}
