<script lang="ts">
  /**
   * In-app preview overlay for text content (Markdown, code, logs…): a
   * calm reading surface with the content's identity up top. Attachment
   * sources get an icon-only "open externally" action as the escape
   * hatch; inline sources (e.g. session rules) have no file behind
   * them, so the action is omitted. Markdown renders through the chat's
   * TextBlock; other text is syntax-highlighted via the shared shiki
   * singleton (plain-text fallback). Attachment bytes come from the
   * daemon over the wire, so local and remote mode behave the same;
   * unreadable/binary files degrade to the external-open hint.
   */
  import { ExternalLink, FileText, Loader2, X } from "lucide-svelte";
  import { errorMessage, openAttachment, readAttachmentText } from "../../api";
  import { closeFilePreview, filePreview } from "../../file-preview.svelte";
  import { showNotification } from "../../state.svelte";
  import { detectLang } from "../../utils";
  import TextBlock from "../chat/TextBlock.svelte";
  import { highlightCode } from "../chat/code-highlight";
  import { isTopModal, pushModal } from "../../modal-stack";

  const overlayId = Symbol("file-preview");

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.parentNode?.removeChild(node);
      },
    };
  }

  type LoadState =
    | { kind: "loading" }
    | { kind: "error"; message: string }
    | { kind: "ready"; text: string; highlighted: string | null };

  const target = $derived(filePreview.target);

  let load = $state<LoadState>({ kind: "loading" });
  let openingExternal = $state(false);

  $effect(() => {
    const t = target;
    if (!t) return;
    load = { kind: "loading" };
    openingExternal = false;
    let cancelled = false;
    void (async () => {
      try {
        const text =
          t.source.kind === "inline"
            ? t.source.text
            : (await readAttachmentText(t.source.base_dir, t.source.path)).text;
        if (cancelled) return;
        if (t.markdown) {
          load = { kind: "ready", text, highlighted: null };
          return;
        }
        const name = t.source.kind === "attachment" ? t.source.path : t.name;
        const highlighted = await highlightCode(text, detectLang(name));
        if (!cancelled) load = { kind: "ready", text, highlighted };
      } catch (e) {
        if (!cancelled) load = { kind: "error", message: errorMessage(e) };
      }
    })();
    return () => {
      cancelled = true;
    };
  });

  function handleKeydown(e: KeyboardEvent) {
    // Only when top-most: a layer opened above (e.g. mermaid zoom inside
    // a markdown preview) owns Escape until it closes.
    if (e.key === "Escape" && filePreview.target && isTopModal(overlayId)) {
      e.preventDefault();
      closeFilePreview();
    }
  }

  // Layer registration: the overlay is Escape-eligible only while it is
  // the stack's top entry.
  $effect(() => {
    if (!target) return;
    return pushModal(overlayId);
  });

  // Move focus into the dialog so Tab doesn't walk the background app.
  let dialogEl = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (target && dialogEl) dialogEl.focus();
  });

  async function openExternally() {
    const t = target;
    if (!t || t.source.kind !== "attachment" || openingExternal) return;
    openingExternal = true;
    try {
      await openAttachment(t.source.base_dir, t.source.path);
    } catch (e: unknown) {
      showNotification(errorMessage(e), "error");
    } finally {
      openingExternal = false;
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if target}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    use:portal
    class="fixed inset-0 z-50 flex items-center justify-center bg-overlay p-4 backdrop-blur-sm sm:p-8"
    onclick={(e) => {
      if (e.target === e.currentTarget) closeFilePreview();
    }}
    role="dialog"
    aria-modal="true"
    aria-label={`Preview ${target.name}`}
    tabindex="-1"
    bind:this={dialogEl}
  >
    <div
      class="flex max-h-full w-full max-w-3xl flex-col overflow-hidden rounded-lg border border-border bg-background shadow-2xl"
    >
      <!-- Header: identity + actions -->
      <div
        class="flex shrink-0 items-center gap-2.5 border-b border-border px-4 py-2.5"
      >
        <FileText size={14} class="shrink-0 text-muted-foreground" />
        <div class="min-w-0 flex-1">
          <div class="truncate text-sm font-medium text-foreground">
            {target.name}
          </div>
          {#if target.sub}
            <div
              class="truncate font-mono text-[10px] text-muted-foreground"
              title={target.sub}
            >
              {target.sub}
            </div>
          {/if}
        </div>
        {#if target.source.kind === "attachment"}
          <button
            type="button"
            class="shrink-0 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-70"
            onclick={openExternally}
            disabled={openingExternal}
            title="Open in the system default app"
            aria-label="Open externally"
          >
            {#if openingExternal}
              <Loader2 size={14} class="animate-spin" />
            {:else}
              <ExternalLink size={14} />
            {/if}
          </button>
        {/if}
        <button
          type="button"
          onclick={closeFilePreview}
          aria-label="Close preview"
          title="Close preview"
          class="shrink-0 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        >
          <X size={14} />
        </button>
      </div>

      <!-- Body -->
      <div class="min-h-0 flex-1 overflow-auto">
        {#if load.kind === "loading"}
          <div
            class="flex items-center justify-center gap-2 py-16 text-muted-foreground"
          >
            <Loader2 size={15} class="animate-spin" />
            <span class="text-xs">Loading preview…</span>
          </div>
        {:else if load.kind === "error"}
          <div
            class="flex flex-col items-center gap-1.5 px-6 py-16 text-center"
          >
            <p class="text-xs text-muted-foreground">{load.message}</p>
            <p class="text-[11px] text-muted-foreground/80">
              This file can't be previewed — open it externally instead.
            </p>
          </div>
        {:else if target.markdown}
          <div class="px-5 py-4">
            <TextBlock content={load.text} />
          </div>
        {:else if load.highlighted}
          <div class="file-preview-code">{@html load.highlighted}</div>
        {:else}
          <pre
            class="whitespace-pre-wrap px-5 py-4 font-mono text-xs leading-relaxed text-foreground">{load.text}</pre>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .file-preview-code {
    padding: 0.75rem 0;
    font-size: 0.75rem;
    line-height: 1.625;
    tab-size: 2;
  }
  .file-preview-code :global(pre.shiki) {
    background-color: transparent !important;
    padding: 0 1.25rem;
    overflow-x: auto;
  }
  .file-preview-code :global(.shiki),
  .file-preview-code :global(.shiki span) {
    background-color: transparent !important;
  }
  :global(.dark) .file-preview-code :global(.shiki),
  :global(.dark) .file-preview-code :global(.shiki span) {
    color: var(--shiki-dark) !important;
    background-color: transparent !important;
  }
</style>
