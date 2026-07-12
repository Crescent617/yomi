<script lang="ts">
  import { Mail } from "lucide-svelte";
  import { formatMessageTime } from "../../utils";
  import { userTextForHeight } from "./user-text";
  import UserText from "./UserText.svelte";

  let {
    content,
    created_at,
  }: {
    content: string;
    created_at?: string;
  } = $props();

  let expanded = $state(false);
  const measuredText = $derived(userTextForHeight(content));
  const isLong = $derived(
    measuredText.split("\n").length > 5 || measuredText.length > 400,
  );
</script>

<div
  class="flex min-w-0 items-start gap-2 rounded-md border-l-2 border-info/40 bg-info/5 px-2.5 py-1.5"
  aria-label="Steer message"
  title="Steer message"
>
  <Mail class="mt-0.5 size-3.5 shrink-0 text-info" aria-hidden="true" />
  <div class="min-w-0 flex-1 text-sm leading-5 text-foreground">
    <div class="relative" class:message-collapsed={isLong && !expanded}>
      <div class:truncate={isLong && !expanded}>
        <UserText text={content} compact />
      </div>
      {#if isLong && !expanded}
        <div
          class="pointer-events-none absolute inset-x-0 bottom-0 h-8 bg-linear-to-t from-info/5 to-transparent"
          aria-hidden="true"
        ></div>
      {/if}
    </div>
    {#if isLong}
      <button
        type="button"
        class="relative z-10 mt-0.5 inline-flex cursor-pointer text-xs font-medium text-info hover:underline"
        onclick={() => (expanded = !expanded)}
        aria-expanded={expanded}
      >
        {expanded ? "Collapse" : "Show full message"}
      </button>
    {/if}
  </div>
  {#if created_at}
    <time
      datetime={created_at}
      class="ml-auto shrink-0 text-[10px] leading-5 tabular-nums text-muted-foreground/70"
    >
      {formatMessageTime(created_at)}
    </time>
  {/if}
</div>

<style>
  .truncate {
    max-height: 120px;
    overflow: hidden;
  }

  .message-collapsed {
    margin-bottom: -0.125rem;
  }
</style>
