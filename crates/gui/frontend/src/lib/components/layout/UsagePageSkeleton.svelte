<script lang="ts">
  import LoadingSkeleton from "../ui/LoadingSkeleton.svelte";
</script>

<div class="space-y-6 py-6" aria-hidden="true">
  <div class="space-y-4 rounded-md border border-primary/20 bg-primary/5 p-4">
    <div class="flex items-center justify-between">
      <LoadingSkeleton class="h-4 w-24" />
      <LoadingSkeleton class="h-3 w-20" />
    </div>
    <div class="grid gap-5 md:grid-cols-3">
      <div class="space-y-3">
        <LoadingSkeleton class="h-8 w-36" />
        <LoadingSkeleton class="h-3 w-48" />
      </div>
      <div class="space-y-2 md:col-span-2">
        <LoadingSkeleton class="h-3 w-full" />
        <LoadingSkeleton class="h-3 w-5/6" />
        <LoadingSkeleton class="h-3 w-2/3" />
      </div>
    </div>
  </div>

  <div class="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-5">
    {#each Array(5) as _, index (index)}
      <div class="space-y-3 rounded-md border border-border bg-card p-3">
        <LoadingSkeleton class="h-3 w-16" />
        <LoadingSkeleton class="h-5 w-24" />
        <LoadingSkeleton class="h-2.5 w-12" />
      </div>
    {/each}
  </div>

  <div class="space-y-4 rounded-md border border-border bg-card p-4">
    <div class="flex justify-between">
      <LoadingSkeleton class="h-4 w-32" />
      <LoadingSkeleton class="h-3 w-24" />
    </div>
    <div class="grid h-32 grid-cols-12 items-end gap-1.5">
      {#each [35, 55, 42, 75, 62, 88, 48, 70, 52, 82, 66, 44] as height, index (index)}
        <LoadingSkeleton class="w-full" style={`height: ${height}%`} />
      {/each}
    </div>
  </div>

  <div class="grid grid-cols-2 gap-3 md:grid-cols-4">
    {#each Array(4) as _, index (index)}
      <LoadingSkeleton class="h-24" />
    {/each}
  </div>
</div>
