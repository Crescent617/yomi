<script lang="ts">
  import { X } from "lucide-svelte";
  import { closeImagePreview, imagePreview } from "../../image-preview.svelte";

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.parentNode?.removeChild(node);
      },
    };
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && imagePreview.src) {
      e.preventDefault();
      closeImagePreview();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if imagePreview.src}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    use:portal
    class="fixed inset-0 z-50 flex items-center justify-center bg-overlay backdrop-blur-sm p-4"
    onclick={(e) => {
      if (e.target === e.currentTarget) closeImagePreview();
    }}
    role="dialog"
    aria-label="Image preview"
    tabindex="-1"
  >
    <img
      src={imagePreview.src}
      alt="Preview"
      class="max-h-full max-w-full rounded-lg object-contain shadow-2xl"
    />
    <button
      type="button"
      onclick={closeImagePreview}
      aria-label="Close preview"
      class="absolute right-4 top-4 rounded-full bg-background/80 p-1.5 text-muted-foreground transition-colors hover:text-foreground"
    >
      <X size={16} />
    </button>
  </div>
{/if}
