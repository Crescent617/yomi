<script lang="ts">
  import LoadingSkeleton from "../ui/LoadingSkeleton.svelte";

  // Placeholder exchange shown while the initial message history loads.
  // Widths are deliberately varied so the shimmer reads as conversation.
  const rows = [
    { user: "w-2/5", assistant: ["w-full", "w-11/12", "w-3/5"] },
    { user: "w-1/4", assistant: ["w-full", "w-5/6"] },
    { user: "w-1/3", assistant: ["w-full", "w-2/3"] },
  ];
</script>

<div
  class="flex flex-col gap-6 pt-2"
  role="status"
  aria-label="Loading messages"
>
  {#each rows as row, index (index)}
    <div class="flex flex-col gap-3">
      <LoadingSkeleton rounded="lg" class="ml-auto h-8 {row.user} opacity-70" />
      <div class="flex flex-col gap-2">
        {#each row.assistant as width, line (line)}
          <LoadingSkeleton class="h-3 {width} opacity-60" />
        {/each}
      </div>
    </div>
  {/each}
  <span class="sr-only">Loading messages…</span>
</div>
