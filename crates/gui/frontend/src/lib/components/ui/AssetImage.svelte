<script lang="ts">
  import { resolveAssetUrl } from "../../utils";
  import { previewImage } from "../../image-preview.svelte";

  let {
    src,
    alt = "Image",
  }: {
    src: string;
    alt?: string;
  } = $props();
</script>

{#snippet thumb(resolved: string)}
  <button
    type="button"
    class="rounded-lg cursor-pointer"
    aria-label="Preview image"
    onclick={() => previewImage(resolved)}
  >
    <img
      src={resolved}
      {alt}
      class="h-[200px] w-[200px] rounded-lg border border-border object-cover transition-opacity hover:opacity-90"
    />
  </button>
{/snippet}

{#if src.startsWith("asset://")}
  {#await resolveAssetUrl(src)}
    <div class="w-[200px] h-[200px] rounded-lg bg-muted animate-pulse"></div>
  {:then resolved}
    {@render thumb(resolved)}
  {:catch}
    <div
      class="w-[200px] h-[200px] rounded-lg bg-muted flex items-center justify-center text-xs text-muted-foreground"
    >
      Failed to load image
    </div>
  {/await}
{:else}
  {@render thumb(src)}
{/if}
