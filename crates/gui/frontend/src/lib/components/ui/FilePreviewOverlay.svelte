<script lang="ts">
  /**
   * In-app preview overlay for text attachments (Markdown, code, logs…):
   * a calm reading surface with the file's identity up top and a soft
   * "Open externally" action as the escape hatch. Markdown renders
   * through the chat's TextBlock; other text is syntax-highlighted via
   * the shared shiki singleton (plain-text fallback). Bytes come from
   * the daemon over the wire, so local and remote mode behave the same;
   * unreadable/binary files degrade to the external-open hint.
   */
  import { ExternalLink, FileText, Loader2, X } from "lucide-svelte";
  import { errorMessage, openAttachment, readAttachmentText } from "../../api";
  import { closeFilePreview, filePreview } from "../../file-preview.svelte";
  import { showNotification } from "../../state.svelte";
  import { detectLang } from "../../utils";
  import TextBlock from "../chat/TextBlock.svelte";
  import { highlightCode } from "../chat/code-highlight";

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
  const name = $derived(
    target
      ? (target.path.split(/[/\\]/).filter(Boolean).pop() ?? target.path)
      : "",
  );
  const isMarkdown = $derived(
    !!target && /\.(md|markdown)$/i.test(target.path),
  );

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
        const { text } = await readAttachmentText(t.base_dir, t.path);
        if (cancelled) return;
        if (/\.(md|markdown)$/i.test(t.path)) {
          load = { kind: "ready", text, highlighted: null };
          return;
        }
        const highlighted = await highlightCode(text, detectLang(t.path));
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
    if (e.key === "Escape" && filePreview.target) {
      e.preventDefault();
      closeFilePreview();
    }
  }

  async function openExternally() {
    const t = target;
    if (!t || openingExternal) return;
    openingExternal = true;
    try {
      await openAttachment(t.base_dir, t.path);
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
    aria-label={`Preview ${name}`}
    tabindex="-1"
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
          <div class="truncate text-sm font-medium text-foreground">{name}</div>
          <div
            class="truncate font-mono text-[10px] text-muted-foreground"
            title={target.path}
          >
            {target.path}
          </div>
        </div>
        <button
          type="button"
          class="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-secondary/50 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-70"
          onclick={openExternally}
          disabled={openingExternal}
          title="Open in the system default app"
        >
          {#if openingExternal}
            <Loader2 size={12} class="animate-spin" />
          {:else}
            <ExternalLink size={12} />
          {/if}
          Open externally
        </button>
        <button
          type="button"
          onclick={closeFilePreview}
          aria-label="Close preview"
          class="shrink-0 rounded-md p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        >
          <X size={15} />
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
        {:else if isMarkdown}
          <div class="px-5 py-4">
            <TextBlock content={load.text} />
          </div>
        {:else if load.highlighted}
          <div class="file-preview-code">{@html load.highlighted}</div>
        {:else}
          <pre
            class="px-5 py-4 font-mono text-xs leading-relaxed whitespace-pre-wrap text-foreground">{load.text}</pre>
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
