<script lang="ts">
  import { Clock, ListChecks } from "lucide-svelte";
  import { getActiveSession } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());

  let todoItems = $state<{ id: string; content: string; status: string }[]>([]);
  let expanded = $state(false);
  let loading = $state(false);

  let el: HTMLDivElement;
  let handleEl: HTMLButtonElement;

  let posX = $state(0);
  let posY = $state(0);
  let startX = 0, startY = 0;
  let dragStartPosX = 0, dragStartPosY = 0;
  let hasDragged = false;

  function loadTodos() {
    const id = activeSession?.id;
    if (!id) {
      todoItems = [];
      return;
    }
    loading = true;
    api.getTodos(id).then((result) => {
      todoItems = result.todos ?? [];
    }).catch(() => {
      todoItems = [];
    }).finally(() => {
      loading = false;
    });
  }

  $effect(() => {
    const _ = activeSession?.id;
    expanded = false;
    loadTodos();
  });

  $effect(() => {
    const _ = activeSession?.messages?.length;
    loadTodos();
  });

  const totalCount = $derived(todoItems.length);
  const completedCount = $derived(todoItems.filter((t) => t.status === "completed").length);
  const inProgressItem = $derived(todoItems.find((t) => t.status === "in_progress"));
  const progressPct = $derived(totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0);
  const hasTodos = $derived(todoItems.length > 0);

  function clampToParent(nextX: number, nextY: number): [number, number] {
    if (!el || !el.parentElement) return [nextX, nextY];
    const eRect = el.getBoundingClientRect();
    const pRect = el.parentElement.getBoundingClientRect();

    // 计算新的 rect（假设 transform 应用后）
    const newLeft = eRect.left + (nextX - posX);
    const newTop = eRect.top + (nextY - posY);
    const newRight = newLeft + eRect.width;
    const newBottom = newTop + eRect.height;

    let clampedX = nextX;
    let clampedY = nextY;

    if (newLeft < pRect.left) clampedX = posX + (pRect.left - eRect.left);
    if (newTop < pRect.top) clampedY = posY + (pRect.top - eRect.top);
    if (newRight > pRect.right) clampedX = posX + (pRect.right - eRect.right);
    if (newBottom > pRect.bottom) clampedY = posY + (pRect.bottom - eRect.bottom);

    return [clampedX, clampedY];
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return; // 只响应左键
    handleEl.setPointerCapture(e.pointerId);
    startX = e.clientX;
    startY = e.clientY;
    dragStartPosX = posX;
    dragStartPosY = posY;
    hasDragged = false;
  }

  function onPointerMove(e: PointerEvent) {
    if (!handleEl.hasPointerCapture(e.pointerId)) return;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    if (Math.abs(dx) > 3 || Math.abs(dy) > 3) {
      hasDragged = true;
    }
    if (hasDragged) {
      let nextX = dragStartPosX + dx;
      let nextY = dragStartPosY + dy;
      [nextX, nextY] = clampToParent(nextX, nextY);
      posX = nextX;
      posY = nextY;
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (handleEl.hasPointerCapture(e.pointerId)) {
      handleEl.releasePointerCapture(e.pointerId);
    }
    if (!hasDragged) {
      expanded = !expanded;
    }
  }
</script>

{#if hasTodos && totalCount !== completedCount}
  <div bind:this={el} class="absolute left-1/2 top-2 z-50 select-none" style="transform: translateX(-50%) translate({posX}px, {posY}px)">
    <div class="flex flex-col items-center gap-1">
      <button
        type="button"
        bind:this={handleEl}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        class="flex items-center gap-2 px-3 py-1.5 rounded-full bg-background border border-border/80 shadow-sm hover:bg-background hover:border-border transition-all text-xs group cursor-move"
      >
        {#if loading}
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
      </button>

      {#if expanded}
        <div class="bg-background border border-border rounded-xl shadow-lg overflow-hidden">
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
    </div>
  </div>
{/if}
