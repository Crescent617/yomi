<script lang="ts" module>
  // Stack of currently-open modals so global key handling (Escape/Enter)
  // only ever targets the top-most one.
  const modalStack: symbol[] = [];
</script>

<script lang="ts">
  import { X } from "lucide-svelte";
  import type { Snippet } from "svelte";

  interface Props {
    open: boolean;
    title?: string;
    showClose?: boolean;
    size?: "sm" | "md" | "lg" | "xl";
    /** When true, the body doesn't scroll or pad itself — the content manages
     *  its own layout inside a flex column (e.g. sticky toolbars). */
    fitContent?: boolean;
    onClose?: () => void;
    /** Invoked when Enter is pressed while this modal is top-most. */
    onSubmit?: () => void;
    children?: Snippet;
    header?: Snippet;
    footer?: Snippet;
  }

  let {
    open,
    title,
    showClose = true,
    size = "md",
    fitContent = false,
    onClose,
    onSubmit,
    children,
    header,
    footer,
  }: Props = $props();

  const sizeClasses = {
    sm: "max-w-sm",
    md: "max-w-lg",
    lg: "max-w-2xl",
    xl: "max-w-4xl",
  };

  const id = Symbol("modal");
  const isTop = () => modalStack[modalStack.length - 1] === id;

  $effect(() => {
    if (!open) return;
    modalStack.push(id);
    return () => {
      const idx = modalStack.indexOf(id);
      if (idx !== -1) modalStack.splice(idx, 1);
    };
  });

  function handleKeydown(e: KeyboardEvent) {
    if (!open || !isTop()) return;
    // Key auto-repeat (a held key) must not re-fire submit/close: one
    // physical keypress, one decision.
    if (e.repeat) return;
    if (e.key === "Escape") {
      e.preventDefault();
      onClose?.();
    } else if (e.key === "Enter" && onSubmit) {
      const target = e.target as HTMLElement | null;
      // Don't hijack Enter from text inputs.
      if (target?.matches("textarea, input, select, [contenteditable]")) return;
      e.preventDefault();
      onSubmit();
    }
  }

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.parentNode?.removeChild(node);
      },
    };
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    use:portal
    class="fixed inset-0 z-50 flex items-center justify-center bg-overlay backdrop-blur-sm p-4"
    onclick={(e) => {
      if (e.target === e.currentTarget) onClose?.();
    }}
  >
    <div
      class="bg-background rounded-xl border border-border shadow-2xl w-full {sizeClasses[
        size
      ]} flex flex-col max-h-[90vh]"
    >
      {#if header || title}
        <div
          class="flex items-center justify-between px-5 py-4 border-b border-border shrink-0"
        >
          {#if header}
            {@render header()}
          {:else}
            <h2 class="text-base font-semibold text-foreground">{title}</h2>
          {/if}
          {#if showClose && onClose}
            <button
              type="button"
              onclick={onClose}
              class="p-1 rounded hover:bg-secondary text-muted-foreground transition-colors"
            >
              <X class="w-5 h-5" />
            </button>
          {/if}
        </div>
      {/if}

      <div
        class="flex-1 min-h-0 {fitContent
          ? 'flex flex-col overflow-hidden'
          : 'overflow-y-auto px-5 py-4'}"
      >
        {@render children?.()}
      </div>

      {#if footer}
        <div
          class="flex items-center justify-end gap-2 px-5 py-4 border-t border-border shrink-0"
        >
          {@render footer()}
        </div>
      {/if}
    </div>
  </div>
{/if}
