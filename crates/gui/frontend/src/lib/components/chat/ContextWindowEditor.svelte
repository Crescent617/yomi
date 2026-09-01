<script lang="ts">
  import {
    errorMessage,
    getSessionContextWindow,
    setSessionContextWindow,
    type ContextWindowInfo,
  } from "../../api";
  import { formatTokens, parseTokenCount } from "../../utils";
  import type { Snippet } from "svelte";

  /**
   * Session context-window editor: the ctx meter in the input toolbar is the
   * trigger (passed in as a snippet); the popover offers presets keyed to the
   * model's configured window, a custom value, and reset. Semantics see
   * docs/design/session-context-window.md.
   */
  interface Props {
    session_id: string;
    /** Latest known info (parent caches it); refreshed on open and after
     *  every applied change. */
    info: ContextWindowInfo | null;
    /** The meter button; called with the toggle callback. */
    trigger: Snippet<[toggle: () => void]>;
  }
  let { session_id, info = $bindable(), trigger }: Props = $props();

  let open = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let custom = $state("");
  let popoverRef: HTMLDivElement | undefined = $state();

  const presets = $derived.by(() => {
    const current = info;
    if (!current) return [];
    return [25, 50, 75, 100].map((p) => ({
      label: `${p}%`,
      tokens: Math.round((current.model_default * p) / 100),
    }));
  });

  /** Accepts `512k`, `1m`, or a plain token count (≤ u32::MAX). */
  function parseTokens(s: string): number | null {
    return parseTokenCount(s);
  }

  async function refresh() {
    const sid = session_id;
    try {
      const fresh = await getSessionContextWindow(sid);
      if (session_id !== sid) return; // session switched mid-flight
      info = fresh;
      error = null;
    } catch (e) {
      if (session_id !== sid) return;
      error = errorMessage(e);
    }
  }

  function toggle() {
    open = !open;
    error = null;
    if (open) void refresh();
  }

  // 切换 session：收起并重置本地态（info 由父组件重置）。
  $effect(() => {
    session_id;
    open = false;
    error = null;
    custom = "";
  });

  async function apply(tokens: number | null) {
    if (busy) return;
    busy = true;
    error = null;
    // set 与 refresh 的错误分开：set 成功但 refresh 失败时变更其实已
    // 生效（info 暂留旧值，下次打开即真）——不能报成失败。
    try {
      await setSessionContextWindow(session_id, tokens);
    } catch (e) {
      error = errorMessage(e);
      busy = false;
      return;
    }
    custom = "";
    try {
      const sid = session_id;
      const fresh = await getSessionContextWindow(sid);
      if (session_id === sid) info = fresh;
    } catch {
      /* applied; the next open refetches */
    }
    busy = false;
  }

  function applyCustom() {
    const tokens = parseTokens(custom);
    if (tokens === null) {
      error = `Invalid value "${custom}" — try 512k, 1m, or a plain number (≤ 4294967295)`;
      return;
    }
    void apply(tokens);
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (open && popoverRef && !popoverRef.contains(target)) {
      // The trigger button handles its own toggle; closing here would fight it.
      if (target.closest("[data-ctx-editor-trigger]")) return;
      open = false;
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div class="relative">
  <span data-ctx-editor-trigger class="inline-flex">
    {@render trigger(toggle)}
  </span>

  {#if open}
    <div
      bind:this={popoverRef}
      class="absolute bottom-full right-0 z-20 mb-1 w-64 rounded-md border border-border bg-popover p-3 shadow-md"
    >
      {#if info}
        <div class="flex items-baseline justify-between gap-2">
          <span class="text-xs font-medium text-foreground"
            >{formatTokens(info.effective)} ctx</span
          >
          <span
            class="rounded-sm px-1.5 py-0.5 text-[10px] {info.override !==
            null
              ? 'bg-secondary text-foreground'
              : 'text-muted-foreground'}"
          >
            {info.override !== null ? "override" : "model default"}
          </span>
        </div>
        <p class="mt-1 text-[10px] text-muted-foreground">
          model `{info.model_key}` default {formatTokens(info.model_default)}
          {#if info.override !== null}
            · compaction follows the override
          {/if}
        </p>

        <div class="mt-2.5 grid grid-cols-4 gap-1">
          {#each presets as p (p.label)}
            <button
              type="button"
              disabled={busy}
              onclick={() => void apply(p.tokens)}
              class="rounded-sm border border-border px-1 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground disabled:opacity-50"
              title={`Set context window to ${formatTokens(p.tokens)} (${p.label} of the model default)`}
            >
              {p.label}
            </button>
          {/each}
        </div>

        <div class="mt-1.5 flex items-center gap-1">
          <input
            bind:value={custom}
            placeholder="512k / 1m"
            disabled={busy}
            onkeydown={(e) => e.key === "Enter" && applyCustom()}
            class="min-w-0 flex-1 rounded-sm border border-border bg-background px-1.5 py-1 text-[11px] text-foreground placeholder:text-muted-foreground/60 focus:outline-none focus:ring-1 focus:ring-ring"
          />
          <button
            type="button"
            disabled={busy || !custom.trim()}
            onclick={applyCustom}
            class="shrink-0 rounded-sm border border-border px-2 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground disabled:opacity-50"
          >
            Set
          </button>
        </div>

        {#if info.override !== null}
          <button
            type="button"
            disabled={busy}
            onclick={() => void apply(null)}
            class="mt-1.5 w-full rounded-sm px-1 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground disabled:opacity-50"
          >
            Reset to model default
          </button>
        {/if}
      {:else}
        <p class="text-[11px] text-muted-foreground">Loading…</p>
      {/if}
      {#if error}
        <p class="mt-1.5 text-[10px] text-error">{error}</p>
      {/if}
    </div>
  {/if}
</div>
