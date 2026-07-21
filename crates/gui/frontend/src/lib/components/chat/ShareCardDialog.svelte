<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { Copy, Check, Save, LoaderCircle } from "lucide-svelte";
  import Modal from "../ui/Modal.svelte";
  import { shareState, closeShare } from "../../share.svelte";
  import { showNotification } from "../../state.svelte";
  import {
    renderShareCard,
    saveShareCard,
    copyShareCardToClipboard,
    MIN_CARD_WIDTH,
    MAX_CARD_WIDTH,
  } from "../../share-card";

  const DEFAULT_WIDTH = 720;

  let width = $state(DEFAULT_WIDTH);
  let previewUrl = $state<string | null>(null);
  let blob = $state<Blob | null>(null);
  let rendering = $state(false);
  let copying = $state(false);
  let saving = $state(false);
  let copied = $state(false);
  let renderTimer: ReturnType<typeof setTimeout> | undefined;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  let renderSeq = 0;

  const request = $derived(shareState.request);

  function setPreview(next: Blob | null) {
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    blob = next;
    previewUrl = next ? URL.createObjectURL(next) : null;
  }

  async function render() {
    const req = shareState.request;
    if (!req) return;
    const seq = ++renderSeq;
    rendering = true;
    try {
      const next = await renderShareCard({ ...req, width });
      if (seq !== renderSeq) return; // superseded by a newer render
      setPreview(next);
    } catch (e) {
      console.error("Failed to render share card:", e);
      if (seq === renderSeq) {
        showNotification("Failed to render share card", "error");
      }
    } finally {
      if (seq === renderSeq) rendering = false;
    }
  }

  // Render once per request; width changes go through onWidthInput.
  $effect(() => {
    if (request) {
      untrack(() => {
        width = DEFAULT_WIDTH;
        void render();
      });
    } else {
      untrack(() => setPreview(null));
    }
  });

  onDestroy(() => {
    clearTimeout(renderTimer);
    clearTimeout(copyTimer);
    if (previewUrl) URL.revokeObjectURL(previewUrl);
  });

  function onWidthInput() {
    clearTimeout(renderTimer);
    renderTimer = setTimeout(() => void render(), 250);
  }

  async function onCopy() {
    if (!blob || copying) return;
    copying = true;
    try {
      await copyShareCardToClipboard(blob);
      clearTimeout(copyTimer);
      copied = true;
      // Brief check-mark feedback, then close.
      copyTimer = setTimeout(() => closeShare(), 500);
    } catch (e) {
      console.error("Failed to copy image:", e);
      showNotification("Failed to copy image", "error");
    } finally {
      copying = false;
    }
  }

  async function onSave() {
    if (!blob || saving) return;
    saving = true;
    try {
      const path = await saveShareCard(blob);
      if (path) {
        showNotification("Share image saved", "success");
        closeShare();
      }
    } catch (e) {
      console.error("Failed to save share image:", e);
      showNotification("Failed to save share image", "error");
    } finally {
      saving = false;
    }
  }
</script>

<Modal
  open={request !== null}
  title="Share answer"
  size="lg"
  onClose={closeShare}
>
  <div class="flex flex-col gap-4">
    <div class="flex items-center gap-3">
      <span class="shrink-0 text-xs text-muted-foreground">Width</span>
      <input
        type="range"
        min={MIN_CARD_WIDTH}
        max={MAX_CARD_WIDTH}
        step="40"
        bind:value={width}
        oninput={onWidthInput}
        aria-label="Card width"
        class="flex-1 accent-primary"
      />
      <span
        class="w-14 shrink-0 text-right text-xs tabular-nums text-muted-foreground"
        >{width}px</span
      >
    </div>

    <div class="relative flex min-h-44 overflow-auto" style="max-height: 55vh">
      {#if previewUrl}
        <img
          src={previewUrl}
          alt="Share card preview"
          class="m-auto h-auto max-w-full rounded-md shadow"
          style="width: {width}px"
        />
      {:else}
        <div class="m-auto py-16 text-muted-foreground">
          <LoaderCircle class="size-5 animate-spin" />
        </div>
      {/if}
      {#if rendering && previewUrl}
        <div
          class="absolute inset-0 flex items-center justify-center bg-overlay/40"
        >
          <LoaderCircle class="size-5 animate-spin text-foreground" />
        </div>
      {/if}
    </div>
  </div>

  {#snippet footer()}
    <button
      type="button"
      onclick={closeShare}
      class="px-4 py-2 rounded-lg text-sm border border-input text-foreground hover:bg-secondary transition-colors"
    >
      Cancel
    </button>
    <button
      type="button"
      onclick={onCopy}
      disabled={!blob || copying || rendering}
      class="inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm border border-input bg-secondary/60 text-foreground hover:bg-secondary transition-colors disabled:opacity-50"
    >
      {#if copying}
        <LoaderCircle class="size-4 animate-spin" />
      {:else if copied}
        <Check class="size-4 text-success" />
      {:else}
        <Copy class="size-4" />
      {/if}
      Copy image
    </button>
    <button
      type="button"
      onclick={onSave}
      disabled={!blob || saving || rendering}
      class="inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
    >
      {#if saving}
        <LoaderCircle class="size-4 animate-spin" />
      {:else}
        <Save class="size-4" />
      {/if}
      Save PNG
    </button>
  {/snippet}
</Modal>
