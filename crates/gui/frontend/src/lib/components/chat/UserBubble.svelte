<script lang="ts">
  import { textFromBlocks } from "../../session";
  import type { UserMessage } from "../../state.svelte";
  import { resolveAssetUrl } from "../../utils";
  import OperationBar from "./OperationBar.svelte";
  import { userTextForHeight } from "./user-text";
  import UserText from "./UserText.svelte";

  let { message, session_id }: { message: UserMessage; session_id: string } =
    $props();

  const text = $derived(textFromBlocks(message.content));
  const measuredText = $derived(userTextForHeight(text));

  let expanded = $state(false);

  const isLong = $derived(
    measuredText.split("\n").length > 5 || measuredText.length > 400,
  );

  const hasImages = $derived(
    message.content.some((b) => b.type === "image_url" && b.image_url?.url),
  );
</script>

<div class="flex justify-end group">
  <div
    class="max-w-[80%] lg:max-w-[70%] rounded-2xl rounded-br-sm bg-secondary px-4 py-3 text-sm space-y-2 relative"
  >
    <!-- Images -->
    {#if hasImages}
      <div class="flex flex-wrap gap-2">
        {#each message.content as block (block.type + (block.image_url?.url ?? block.text ?? ""))}
          {#if block.type === "image_url" && block.image_url?.url}
            {#if block.image_url.url.startsWith("asset://")}
              {#await resolveAssetUrl(block.image_url.url)}
                <div
                  class="w-[200px] h-[200px] rounded-lg bg-muted animate-pulse"
                ></div>
              {:then src}
                <button
                  type="button"
                  class="rounded-lg"
                  aria-label="Open uploaded image in a new tab"
                  onclick={() => window.open(src, "_blank")}
                >
                  <img
                    {src}
                    alt="Uploaded attachment"
                    class="max-w-[200px] max-h-[200px] rounded-lg object-cover border border-border cursor-pointer hover:opacity-90 transition-opacity"
                  />
                </button>
              {:catch}
                <div
                  class="w-[200px] h-[200px] rounded-lg bg-muted flex items-center justify-center text-xs text-muted-foreground"
                >
                  Failed to load image
                </div>
              {/await}
            {:else}
              <button
                type="button"
                class="rounded-lg"
                aria-label="Open uploaded image in a new tab"
                onclick={() => window.open(block.image_url!.url, "_blank")}
              >
                <img
                  src={block.image_url.url}
                  alt="Uploaded attachment"
                  class="max-w-[200px] max-h-[200px] rounded-lg object-cover border border-border cursor-pointer hover:opacity-90 transition-opacity"
                />
              </button>
            {/if}
          {/if}
        {/each}
      </div>
    {/if}

    <!-- Text content -->
    {#if text.trim()}
      <div class="relative" class:message-collapsed={isLong && !expanded}>
        <div class:truncate={isLong && !expanded}>
          <UserText {text} />
        </div>
        {#if isLong && !expanded}
          <div
            class="pointer-events-none absolute inset-x-0 bottom-0 h-12 bg-linear-to-t from-secondary to-transparent"
            aria-hidden="true"
          ></div>
        {/if}
      </div>
      {#if isLong}
        <button
          type="button"
          class="relative z-10 mt-1 inline-flex text-xs font-medium text-primary hover:underline cursor-pointer"
          onclick={() => (expanded = !expanded)}
          aria-expanded={expanded}
        >
          {expanded ? "Collapse" : "Show full message"}
        </button>
      {/if}
    {/if}
    <div
      class="message-actions absolute right-full top-0 z-10 mr-1.5 translate-x-1 opacity-0 transition-[opacity,transform] duration-150 group-hover:translate-x-0 group-hover:opacity-100 group-focus-within:translate-x-0 group-focus-within:opacity-100"
    >
      <OperationBar {message} {session_id} />
    </div>
  </div>
</div>

<style>
  @media (hover: none) {
    .message-actions {
      opacity: 1;
      transform: translateX(0);
    }
  }

  .truncate {
    max-height: 120px;
    overflow: hidden;
  }
  .message-collapsed {
    margin-bottom: -0.25rem;
  }
</style>
