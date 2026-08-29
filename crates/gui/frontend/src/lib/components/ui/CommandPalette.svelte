<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Search } from "lucide-svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    closePalette,
    commandRows,
    openPalette,
    paletteInCommandMode,
    paletteState,
    sessionRows,
    type PaletteRow,
  } from "../../command-palette.svelte";

  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLElement | null>(null);
  // Focus returns to whatever opened the palette (modal cycle).
  let restoreFocus: HTMLElement | null = null;

  const commandMode = $derived(paletteInCommandMode());
  const rows = $derived(commandMode ? commandRows() : sessionRows());
  // Flat display list: group headers interleaved with row indices, so a
  // row's absolute index is computed once (no per-render indexOf scan).
  type DisplayItem =
    | { kind: "header"; name: string }
    | { kind: "row"; row: PaletteRow; index: number };
  const displayItems = $derived.by(() => {
    const items: DisplayItem[] = [];
    let lastGroup = "";
    rows.forEach((row, index) => {
      if (row.group !== lastGroup) {
        lastGroup = row.group;
        items.push({ kind: "header", name: row.group });
      }
      items.push({ kind: "row", row, index });
    });
    return items;
  });

  // Clamp the cursor whenever the row set changes under the same query.
  $effect(() => {
    if (paletteState.selected >= rows.length) {
      paletteState.selected = Math.max(0, rows.length - 1);
    }
  });

  // Fresh query → back to the top row.
  $effect(() => {
    void paletteState.query;
    paletteState.selected = 0;
  });

  // Focus the input on open; hand focus back on close.
  $effect(() => {
    if (paletteState.open) {
      void tick().then(() => inputEl?.focus());
    } else if (restoreFocus) {
      const el = restoreFocus;
      restoreFocus = null;
      void tick().then(() => {
        if (el.isConnected) el.focus();
      });
    }
  });

  // Keep the selected row visible while keyboard-navigating.
  $effect(() => {
    const i = paletteState.selected;
    void tick().then(() => {
      listEl
        ?.querySelector(`[data-row-index="${i}"]`)
        ?.scrollIntoView({ block: "nearest" });
    });
  });

  function onGlobalKeydown(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    // ⌘P is inert while the confirm dialog owns the screen: opening the
    // palette behind it would focus an invisible input and stack two
    // aria-modal dialogs.
    if (mod && e.key.toLowerCase() === "p") {
      e.preventDefault(); // browser/e2e: no print dialog, confirm or not
      if (paletteState.confirm) return;
      if (paletteState.open) {
        closePalette();
      } else {
        restoreFocus =
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null;
        openPalette(e.shiftKey);
      }
    }
  }

  // Esc in CAPTURE phase: the palette closes itself and consumes the key
  // before any bubble-phase window listener (an underlying Modal's own
  // Esc) can see it — one Esc never closes two layers. stopPropagation
  // also keeps the palette's own input out of it, which is fine: nothing
  // in the palette needs Esc for itself.
  onMount(() => {
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !paletteState.open || paletteState.confirm) {
        return;
      }
      e.stopPropagation();
      closePalette();
    };
    window.addEventListener("keydown", onEsc, true);
    return () => window.removeEventListener("keydown", onEsc, true);
  });

  function runRow(row: PaletteRow) {
    closePalette();
    row.run();
  }

  function onInputKeydown(e: KeyboardEvent) {
    // IME composition (pinyin etc.): Enter/arrow keys belong to the
    // candidate window, never to the palette.
    if (e.isComposing) return;
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        paletteState.selected = Math.min(
          rows.length - 1,
          paletteState.selected + 1,
        );
        break;
      case "ArrowUp":
        e.preventDefault();
        paletteState.selected = Math.max(0, paletteState.selected - 1);
        break;
      case "Home":
        e.preventDefault();
        paletteState.selected = 0;
        break;
      case "End":
        e.preventDefault();
        paletteState.selected = Math.max(0, rows.length - 1);
        break;
      case "Enter": {
        e.preventDefault();
        const row = rows[paletteState.selected];
        if (row) runRow(row);
        break;
      }
    }
  }
</script>

<svelte:window onkeydown={onGlobalKeydown} />

{#if paletteState.open}
  <div
    class="fixed inset-0 z-50"
    role="dialog"
    aria-modal="true"
    aria-label="命令面板"
  >
    <div
      class="absolute inset-0 bg-overlay backdrop-blur-sm"
      onclick={closePalette}
      role="presentation"
    ></div>

    <div
      class="absolute left-1/2 top-[12vh] w-[min(38rem,92vw)] -translate-x-1/2 overflow-hidden rounded-xl border border-border bg-background shadow-2xl"
    >
      <!-- Input -->
      <div class="flex items-center gap-2.5 border-b border-border px-3.5">
        <Search class="size-[15px] shrink-0 text-muted-foreground" />
        <input
          bind:this={inputEl}
          bind:value={paletteState.query}
          onkeydown={onInputKeydown}
          class="flex-1 bg-transparent py-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground"
          placeholder="搜索会话名或 ID…输入 > 进入命令"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-rows"
          aria-activedescendant={rows[paletteState.selected]
            ? `palette-row-${paletteState.selected}`
            : undefined}
          aria-label={commandMode ? "搜索命令" : "搜索会话"}
          spellcheck="false"
          autocomplete="off"
        />
        <kbd
          class="micro-label rounded border border-border px-1.5 py-0.5 text-muted-foreground"
          >esc</kbd
        >
      </div>

      <!-- Rows -->
      <div
        bind:this={listEl}
        id="palette-rows"
        class="max-h-[min(60vh,30rem)] overflow-y-auto py-1"
        role="listbox"
        aria-label={commandMode ? "命令" : "会话"}
      >
        {#if rows.length === 0}
          <p class="px-3.5 py-6 text-center text-sm text-muted-foreground">
            {commandMode ? "没有匹配的命令" : "没有匹配的会话"}
          </p>
        {/if}
        {#each displayItems as item (item.kind === "header" ? `h-${item.name}` : item.row.key)}
          {#if item.kind === "header"}
            <p class="micro-label px-3.5 pb-1 pt-2.5 text-muted-foreground">
              {item.name}
            </p>
          {:else}
            <button
              type="button"
              role="option"
              id={`palette-row-${item.index}`}
              aria-selected={item.index === paletteState.selected}
              data-row-index={item.index}
              class="flex w-full items-center gap-2.5 px-3.5 py-1.5 text-left transition-colors focus-visible:outline-none {item.index ===
              paletteState.selected
                ? 'bg-primary/10'
                : ''}"
              onmousemove={(e) => {
                // Scroll-driven mousemove (keyboard nav scrolling rows
                // under a stationary cursor) carries zero movement —
                // only real mouse moves may steal the selection.
                if (e.movementX === 0 && e.movementY === 0) return;
                paletteState.selected = item.index;
              }}
              onclick={() => runRow(item.row)}
            >
              <item.row.icon
                class="size-[15px] shrink-0 {item.row.danger
                  ? 'text-error'
                  : 'text-muted-foreground'}"
              />
              <span
                class="flex-1 truncate text-sm {item.row.danger
                  ? 'text-error'
                  : 'text-foreground'}">{item.row.title}</span
              >
              {#if item.row.hint}
                <span
                  class="max-w-[45%] truncate font-mono text-[11px] text-muted-foreground"
                  >{item.row.hint}</span
                >
              {/if}
            </button>
          {/if}
        {/each}
      </div>

      <!-- Footer -->
      <div
        class="micro-label flex items-center gap-3 border-t border-border px-3.5 py-1.5 text-muted-foreground"
      >
        <span>↑↓ 导航</span>
        <span>Enter {commandMode ? "执行" : "跳转"}</span>
        <span>Esc 关闭</span>
        {#if !commandMode}
          <span class="ml-auto">&gt; 命令模式</span>
        {/if}
      </div>
    </div>
  </div>
{/if}

<ConfirmDialog
  open={paletteState.confirm !== null}
  title={paletteState.confirm?.title ?? ""}
  message={paletteState.confirm?.message ?? ""}
  confirmText={paletteState.confirm?.confirmText ?? "确认"}
  onConfirm={() => {
    const action = paletteState.confirm?.action;
    paletteState.confirm = null;
    void action?.();
  }}
  onCancel={() => (paletteState.confirm = null)}
/>
