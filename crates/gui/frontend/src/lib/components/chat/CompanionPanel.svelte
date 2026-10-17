<script lang="ts">
  // 伙伴窗对话面板：标题栏（绑定 session + 操作）+ 消息流 + 权限/提问条
  // + slim 输入。消息与权限状态全部来自本窗口的 session store，绑定切换
  // 由 companion.svelte.ts 驱动。
  import { ExternalLink, Pin, PinOff, X } from "lucide-svelte";
  import * as api from "../../api";
  import { showNotification } from "../../state.svelte";
  import { isActiveSessionPhase } from "../../session-phase";
  import {
    boundSession,
    boundSessionId,
    closeCompanion,
    companionState,
    togglePinnedSession,
  } from "../../companion.svelte";
  import MessageList from "./MessageList.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import CompanionInput from "./CompanionInput.svelte";

  const session = $derived(boundSession());
  const session_id = $derived(boundSessionId());
  const title = $derived(session?.alias ?? session_id ?? null);
  const pinned = $derived(companionState.dto?.pinned_session_id != null);
  const running = $derived(
    session ? isActiveSessionPhase(session.phase) : false,
  );
  const disconnected = $derived(
    companionState.snapshot?.connection_status === "disconnected",
  );

  let input = $state<CompanionInput | undefined>();
  let wrap_el = $state<HTMLDivElement | undefined>();

  export function focusInput() {
    input?.focus();
  }

  // ── 内容高度自适应 ────────────────────────────────────────────────────
  // 量 section 各子项自然高度之和（消息区用 scroller 内内容高），报给
  // Rust 钳 [240,560] 并重算窗口框架。消息增长、流式变长、权限条出现都
  // 会触发；4px 死区防抖动。

  const REPORT_DEBOUNCE_MS = 100;
  const HEIGHT_DEADZONE_PX = 4;

  function measureContentHeight(): number {
    const section = wrap_el?.querySelector("section");
    if (!section) return 0;
    let total = 0;
    for (const child of Array.from(section.children)) {
      if (!(child instanceof HTMLElement)) continue;
      if (child.dataset.role === "messages") {
        const scroller = child.querySelector(".overflow-y-auto");
        const content = scroller?.firstElementChild;
        const toolbar = scroller?.parentElement?.previousElementSibling;
        total +=
          (toolbar instanceof HTMLElement ? toolbar.offsetHeight : 0) +
          (content instanceof HTMLElement
            ? content.offsetHeight
            : (scroller?.scrollHeight ?? child.offsetHeight));
      } else {
        total += child.offsetHeight;
      }
    }
    return total;
  }

  $effect(() => {
    if (!wrap_el) return;
    const wrap = wrap_el;
    let last_sent = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const report = () => {
      const total = measureContentHeight();
      if (total <= 0) return;
      if (Math.abs(total - last_sent) < HEIGHT_DEADZONE_PX) return;
      last_sent = total;
      void api.setCompanionPanelHeight(total).catch(() => {});
    };
    const schedule = () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(report, REPORT_DEBOUNCE_MS);
    };

    const resize_observer = new ResizeObserver(schedule);
    resize_observer.observe(wrap);
    const observed = new Set<Element>();
    const attach = () => {
      const section = wrap.querySelector("section");
      if (!section) return;
      for (const child of Array.from(section.children)) {
        if (!observed.has(child)) {
          observed.add(child);
          resize_observer.observe(child);
        }
      }
      const content = section.querySelector(
        '[data-role="messages"] .overflow-y-auto > *',
      );
      if (content && !observed.has(content)) {
        observed.add(content);
        resize_observer.observe(content);
      }
    };
    attach();
    const mutation_observer = new MutationObserver(() => {
      attach();
      schedule();
    });
    const section = wrap.querySelector("section");
    // subtree: 消息区内部（skeleton→正文、消息追加、绑定切换重建）也会
    // 换出被观察的 content 节点；只在挂载时存在的节点上挂观察会在
    // 空状态开面板后漏装。
    if (section) {
      mutation_observer.observe(section, { childList: true, subtree: true });
    }
    schedule();

    return () => {
      if (timer !== undefined) clearTimeout(timer);
      resize_observer.disconnect();
      mutation_observer.disconnect();
    };
  });

  async function send(text: string) {
    if (!session_id) return;
    try {
      await api.sendMessage(session_id, text);
    } catch (error) {
      showNotification(`Send failed: ${api.errorMessage(error)}`, "error");
      // 重新抛出：CompanionInput 据此保留草稿。
      throw error;
    }
  }

  async function openInMain() {
    if (!session_id) return;
    try {
      await api.openSessionInMain(session_id);
      await closeCompanion();
    } catch (error) {
      showNotification(
        `Open in main window failed: ${api.errorMessage(error)}`,
        "error",
      );
    }
  }
</script>

<div class="panel-wrap" bind:this={wrap_el}>
  <section
    class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-subtle bg-background shadow-lg"
    aria-label="Companion conversation panel"
  >
    <header
      class="group flex items-center gap-1 border-b border-subtle px-2 py-1.5"
      data-tauri-drag-region={false}
    >
      <div class="min-w-0 flex-1 leading-tight">
        <div class="truncate text-xs font-medium">
          {title ?? "No session"}
        </div>
        <div class="micro-label text-muted-foreground">
          {#if disconnected}
            daemon disconnected
          {:else if !session_id}
            follow the active session
          {:else if running}
            working
          {:else}
            idle
          {/if}
        </div>
      </div>
      <button
        type="button"
        onclick={() => void togglePinnedSession()}
        disabled={!session_id && !pinned}
        title={pinned ? "Unpin session" : "Pin this session"}
        aria-label={pinned ? "Unpin session" : "Pin this session"}
        class="rounded p-1 text-muted-foreground transition-opacity hover:text-foreground disabled:opacity-40 {pinned
          ? ''
          : 'opacity-0 group-hover:opacity-100'}"
      >
        {#if pinned}<PinOff size={13} />{:else}<Pin size={13} />{/if}
      </button>
      <button
        type="button"
        onclick={() => void openInMain()}
        disabled={!session_id}
        title="Open in main window"
        aria-label="Open in main window"
        class="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-40"
      >
        <ExternalLink size={13} />
      </button>
      <button
        type="button"
        onclick={() => void closeCompanion()}
        title="Close panel"
        aria-label="Close panel"
        class="rounded p-1 text-muted-foreground hover:text-foreground"
      >
        <X size={13} />
      </button>
    </header>

    {#if session_id}
      <div class="min-h-0 flex-1" data-role="messages">
        <MessageList />
      </div>
      <PermissionBar />
      <AskUserBar />
      <CompanionInput
        bind:this={input}
        disabled={disconnected}
        placeholder={running
          ? "Queue a message…"
          : `Message ${title ?? "session"}…`}
        onSend={send}
      />
    {:else}
      <div class="flex flex-1 items-center justify-center p-4 text-center">
        <p class="text-xs text-muted-foreground">
          Nothing to follow yet. Activate a session in the main window, then
          click the pet again.
        </p>
      </div>
    {/if}
  </section>
</div>

<style>
  .panel-wrap {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }
</style>
