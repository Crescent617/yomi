<script lang="ts">
  import { getActiveSession, getDisplayMessages } from "../../state.svelte";
  import { ArrowDown, ChevronDown, Clock, ListChecks } from "lucide-svelte";
  import * as api from "../../api";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import SystemBubble from "./SystemBubble.svelte";
  import ErrorBubble from "./ErrorBubble.svelte";

  const activeSession = $derived(getActiveSession());
  const displayMessages = $derived(getDisplayMessages(activeSession?.id ?? ""));

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);

  // ── todo ──
  let todoItems = $state<{ id: string; content: string; status: string }[]>([]);
  let todoExpanded = $state(false);
  let todoLoading = $state(false);

  function loadTodos() {
    const id = activeSession?.id;
    if (!id) {
      todoItems = [];
      return;
    }
    todoLoading = true;
    api.getTodos(id).then((result) => {
      todoItems = result.todos ?? [];
    }).catch(() => {
      todoItems = [];
    }).finally(() => {
      todoLoading = false;
    });
  }

  // Load on session change
  $effect(() => {
    const _ = activeSession?.id;
    todoExpanded = false;
    loadTodos();
  });

  // Refresh when messages change (streaming updates)
  $effect(() => {
    const _ = activeSession?.messages?.length;
    loadTodos();
  });

  const totalCount = $derived(todoItems.length);
  const completedCount = $derived(todoItems.filter((t) => t.status === "completed").length);
  const inProgressItem = $derived(todoItems.find((t) => t.status === "in_progress"));
  const progressPct = $derived(totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0);
  const hasTodos = $derived(todoItems.length > 0);

  function checkNearBottom() {
    if (!scrollContainer) return true;
    const threshold = 80; // px from bottom — relaxed to avoid flicker during streaming
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return scrollHeight - scrollTop - clientHeight <= threshold;
  }

  export function scrollToBottom() {
    if (!scrollContainer) return;
    scrollContainer.scrollTop = scrollContainer.scrollHeight;
    isNearBottom = true;
  }

  // Track last message fingerprint for detecting changes during streaming
  let lastFingerprint = $state("");

  function getFingerprint() {
    if (!displayMessages?.length) return "";
    const last = displayMessages[displayMessages.length - 1];
    const parts: string[] = [last.id, last.role, last.content.length.toString()];
    if (last.thinking) parts.push(last.thinking.content.length.toString());
    if (last.tools) {
      for (const t of last.tools) {
        parts.push(t.id, t.status, (t.output ?? "").length.toString());
      }
    }
    return parts.join("|");
  }

  // Auto-scroll to bottom only when user is already near bottom
  $effect(() => {
    // Capture the fingerprint before anything else — this is the reactive read.
    const fp = getFingerprint();
    // If nothing changed, skip.
    if (fp === lastFingerprint) return;
    // Update the tracked value.  Because we return immediately after, Svelte
    // will not re-run this effect in the same tick (the dependency changed,
    // but there are no further reactive reads after this point).
    lastFingerprint = fp;

    if (scrollContainer && isNearBottom) {
      requestAnimationFrame(() => {
        scrollContainer!.scrollTop = scrollContainer!.scrollHeight;
      });
    }
  });

  function onScroll() {
    isNearBottom = checkNearBottom();
  }
</script>

{#if activeSession}
  <div class="h-full relative">
    <div bind:this={scrollContainer} onscroll={onScroll} class="h-full overflow-y-auto">
      <div class="container mx-auto px-4 lg:px-6 pt-2 pb-4">
        <!-- Sticky todo progress bar -->
        <div class="sticky top-2 z-20 flex flex-col items-center mb-4 relative">
        {#if hasTodos && totalCount !== completedCount}
          <button
            type="button"
            onclick={() => todoExpanded = !todoExpanded}
            class="flex items-center gap-2 px-3 py-1.5 rounded-full bg-background/90 backdrop-blur-sm border border-border/80 shadow-sm hover:bg-background hover:border-border transition-all text-xs group"
          >
            {#if todoLoading}
              <div class="w-3 h-3 border border-primary border-t-transparent rounded-full animate-spin"></div>
            {:else}
              <div class="flex items-center gap-2">
                <ListChecks size={13} class="text-muted-foreground" />
                <span class="text-muted-foreground font-medium tabular-nums">{completedCount}/{totalCount}</span>
                {#if inProgressItem}
                  <div class="h-3 w-px bg-border"></div>
                  <div class="flex items-center gap-1 max-w-[75%]">
                    <Clock size={12} class="text-amber-500 shrink-0 animate-pulse" />
                    <span class="truncate text-foreground">{inProgressItem.content}</span>
                  </div>
                {:else}
                  <div class="w-16 h-1.5 rounded-full bg-muted overflow-hidden">
                    <div class="h-full bg-primary rounded-full transition-all" style="width: {progressPct}%"></div>
                  </div>
                {/if}
              </div>
            {/if}
            <ChevronDown size={12} class="text-muted-foreground transition-transform {todoExpanded ? 'rotate-180' : ''}" />
          </button>
          {#if todoExpanded}
            <div class="absolute top-full mt-2 max-w-[80%] bg-background/95 backdrop-blur-sm border border-border rounded-xl shadow-lg overflow-hidden z-30">
              <div class="max-h-64 overflow-y-auto p-3 space-y-1">
                {#each todoItems as item (item.id)}
                  <div class="flex items-start gap-2 text-sm rounded-lg px-2 py-1.5 hover:bg-secondary/40 transition-colors">
                    <div class="mt-0.5 shrink-0 w-4 h-4 rounded border {item.status === 'completed' ? 'bg-green-500 border-green-500' : item.status === 'in_progress' ? 'border-amber-500' : 'border-muted-foreground'} flex items-center justify-center">
                      {#if item.status === 'completed'}
                        <svg class="w-3 h-3 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="3"><path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" /></svg>
                      {/if}
                    </div>
                    <span class="{item.status === 'completed' ? 'line-through text-muted-foreground' : item.status === 'in_progress' ? 'text-amber-500' : ''}">{item.content}</span>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        {/if}
      </div>

      <div class="space-y-4">
        {#each displayMessages as message, index (message.id)}
          {@const isLastMessage = index === displayMessages.length - 1}
          {@const isStreaming = activeSession.streaming && isLastMessage}
          {#if message.role === "user"}
            <UserBubble {message} />
          {:else if message.error || message.role === "error"}
            <ErrorBubble {message} />
          {:else if message.role === "system"}
            <SystemBubble {message} />
          {:else}
            <AssistantBubble {message} {isStreaming} />
          {/if}
        {/each}
      </div>
    </div>
  </div>
  {#if !isNearBottom}
    <button
      type="button"
      onclick={scrollToBottom}
      class="absolute bottom-4 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-3 py-1.5 rounded-full bg-primary text-primary-foreground text-xs shadow-lg hover:bg-primary/90 transition-colors"
    >
      <ArrowDown class="w-3 h-3" />
      Bottom
    </button>
  {/if}
</div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
