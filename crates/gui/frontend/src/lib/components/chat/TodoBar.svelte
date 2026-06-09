<script lang="ts">
  import { Clock, ListChecks, Target, Pause, Play, Square, Pencil } from "lucide-svelte";
  import { getActiveSession } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());

  let todoItems = $state<{ id: string; content: string; status: string }[]>([]);
  let expanded = $state(false);
  let loadingTodos = $state(false);
  let loadingGoal = $state(false);
  let editingGoal = $state(false);
  let editGoalText = $state("");

  let el: HTMLDivElement;
  let handleEl: HTMLButtonElement;

  let posX = $state(0);
  let posY = $state(0);
  let startX = 0, startY = 0;
  let dragStartPosX = 0, dragStartPosY = 0;
  let hasDragged = false;

  const goal = $derived(activeSession?.goal ?? null);

  function loadTodos() {
    const id = activeSession?.id;
    if (!id) {
      todoItems = [];
      return;
    }
    loadingTodos = true;
    api.getTodos(id).then((result) => {
      todoItems = result.todos ?? [];
    }).catch(() => {
      todoItems = [];
    }).finally(() => {
      loadingTodos = false;
    });
  }

  function loadGoal() {
    const id = activeSession?.id;
    if (!id) return;
    loadingGoal = true;
    const session = activeSession;
    api.getGoal(id).then((result) => {
      if (session) {
        session.goal = result;
      }
    }).catch(() => {
      if (session) {
        session.goal = null;
      }
    }).finally(() => {
      loadingGoal = false;
    });
  }

  $effect(() => {
    const _ = activeSession?.id;
    expanded = false;
    editingGoal = false;
    loadTodos();
    loadGoal();
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
  const hasActiveGoal = $derived(!!goal);
  const shouldShow = $derived(hasActiveGoal || (hasTodos && totalCount !== completedCount));

  function statusDotClass(status: string): string {
    switch (status) {
      case "active": return "bg-green-500 animate-pulse";
      case "paused": return "bg-amber-500";
      case "blocked": return "bg-red-500 animate-pulse";
      case "completed": return "bg-green-500";
      default: return "bg-gray-400";
    }
  }

  function statusBadgeClass(status: string): string {
    switch (status) {
      case "active": return "bg-green-500/10 text-green-500 border-green-500/20";
      case "paused": return "bg-amber-500/10 text-amber-500 border-amber-500/20";
      case "blocked": return "bg-red-500/10 text-red-500 border-red-500/20";
      case "completed": return "bg-green-500/10 text-green-500 border-green-500/20";
      default: return "bg-gray-500/10 text-gray-500 border-gray-500/20";
    }
  }

  async function handlePauseGoal() {
    if (!activeSession?.id || !goal) return;
    await api.pauseGoal(activeSession.id);
    loadGoal();
  }

  async function handleResumeGoal() {
    if (!activeSession?.id || !goal) return;
    await api.resumeGoal(activeSession.id);
    loadGoal();
  }

  async function handleStopGoal() {
    if (!activeSession?.id) return;
    await api.stopGoal(activeSession.id);
    loadGoal();
  }

  function startEditGoal() {
    if (!goal) return;
    editGoalText = goal.description;
    editingGoal = true;
    expanded = true;
  }

  async function submitEditGoal() {
    if (loadingGoal || !activeSession?.id || !editGoalText.trim()) return;
    loadingGoal = true;
    try {
      await api.editGoal(activeSession.id, editGoalText.trim());
      editingGoal = false;
      loadGoal();
    } finally {
      loadingGoal = false;
    }
  }

  function cancelEditGoal() {
    editingGoal = false;
    editGoalText = "";
  }

  function clampToParent(nextX: number, nextY: number): [number, number] {
    if (!el || !el.parentElement) return [nextX, nextY];
    const eRect = el.getBoundingClientRect();
    const pRect = el.parentElement.getBoundingClientRect();

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
    if (e.button !== 0) return;
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

{#if shouldShow}
  <div bind:this={el} class="absolute left-1/2 top-2 z-50 select-none" style="transform: translateX(-50%) translate({posX}px, {posY}px)">
    <div class="flex flex-col items-center gap-1">
      <button
        type="button"
        bind:this={handleEl}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        class="flex items-center gap-3 px-3 py-1.5 rounded-full bg-background border border-border/80 shadow-sm hover:bg-background hover:border-border transition-all text-xs group cursor-move max-w-[80vw]"
      >
        {#if loadingTodos || loadingGoal}
          <div class="w-3 h-3 border border-primary border-t-transparent rounded-full animate-spin"></div>
        {:else}
          <!-- Goal section -->
          {#if hasActiveGoal}
            <div class="flex items-center gap-1.5 shrink-0">
              <Target size={13} class="text-primary shrink-0" />
              <span class="truncate max-w-[180px] text-foreground font-medium" title={goal!.description}>
                {goal!.description}
              </span>
              <span class="w-1.5 h-1.5 rounded-full {statusDotClass(goal!.status)}"></span>
            </div>
          {/if}

          <!-- Separator when both present -->
          {#if hasActiveGoal && hasTodos && totalCount !== completedCount}
            <div class="h-3 w-px bg-border shrink-0"></div>
          {/if}

          <!-- Todo section -->
          {#if hasTodos && totalCount !== completedCount}
            <div class="flex items-center gap-2 shrink-0">
              <ListChecks size={13} class="text-muted-foreground" />
              <span class="text-muted-foreground font-medium tabular-nums">{completedCount}/{totalCount}</span>
              {#if inProgressItem}
                <div class="h-3 w-px bg-border"></div>
                <div class="flex items-center gap-1 max-w-[120px]">
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
        {/if}
      </button>

      {#if expanded}
        <div class="bg-background border border-border rounded-xl shadow-lg overflow-hidden w-80 max-w-[85vw]">
          <!-- Goal Card -->
          {#if hasActiveGoal}
            <div class="p-3.5 border-b border-border">
              <div class="flex items-center justify-between mb-2">
                <div class="flex items-center gap-1.5">
                  <Target size={14} class="text-primary" />
                  <span class="text-xs font-semibold text-foreground uppercase tracking-wide">Goal</span>
                </div>
                <div class="flex items-center gap-1.5">
                  <span class="text-[10px] px-1.5 py-0.5 rounded-full border font-medium uppercase {statusBadgeClass(goal!.status)}">
                    {goal!.status}
                  </span>
                </div>
              </div>

              {#if editingGoal}
                <div class="space-y-2">
                  <textarea
                    bind:value={editGoalText}
                    class="w-full text-sm bg-muted rounded-lg px-2.5 py-2 border border-border focus:outline-none focus:ring-1 focus:ring-ring resize-none"
                    rows={3}
                    onkeydown={(e: KeyboardEvent) => {
                      if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault();
                        submitEditGoal();
                      }
                      if (e.key === 'Escape') cancelEditGoal();
                    }}
                  ></textarea>
                  <div class="flex items-center justify-end gap-2">
                    <button
                      type="button"
                      onclick={cancelEditGoal}
                      class="px-2 py-1 text-xs rounded-md border border-border hover:bg-secondary transition-colors text-muted-foreground"
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      onclick={submitEditGoal}
                      class="px-2 py-1 text-xs rounded-md bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
                    >
                      Save
                    </button>
                  </div>
                </div>
              {:else}
                <p class="text-sm text-foreground leading-relaxed mb-2.5">{goal!.description}</p>
                <div class="flex items-center gap-1.5">
                  {#if goal!.status === "active"}
                    <button
                      type="button"
                      onclick={handlePauseGoal}
                      class="flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-medium border border-border hover:bg-secondary transition-colors text-muted-foreground"
                      title="Pause goal auto-continue"
                    >
                      <Pause size={12} />
                      Pause
                    </button>
                  {:else if goal!.status === "paused"}
                    <button
                      type="button"
                      onclick={handleResumeGoal}
                      class="flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-medium border border-green-500/30 hover:bg-green-500/10 transition-colors text-green-600"
                      title="Resume goal"
                    >
                      <Play size={12} />
                      Resume
                    </button>
                  {/if}
                  <button
                    type="button"
                    onclick={startEditGoal}
                    class="flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-medium border border-border hover:bg-secondary transition-colors text-muted-foreground"
                    title="Edit goal description"
                  >
                    <Pencil size={12} />
                    Edit
                  </button>
                  <button
                    type="button"
                    onclick={handleStopGoal}
                    class="flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-medium border border-red-500/30 hover:bg-red-500/10 transition-colors text-red-500"
                    title="Stop and clear goal"
                  >
                    <Square size={12} />
                    Stop
                  </button>
                </div>
              {/if}
            </div>
          {/if}

          <!-- Todo List -->
          {#if hasTodos}
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
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}
