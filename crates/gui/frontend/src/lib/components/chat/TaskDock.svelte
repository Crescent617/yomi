<script lang="ts">
  import {
    Bot,
    Check,
    CheckCircle2,
    ChevronDown,
    Circle,
    Loader2,
    Pause,
    Pencil,
    Play,
    Square,
    Target,
  } from "lucide-svelte";
  import { onMount } from "svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import RunningSubagents from "./RunningSubagents.svelte";
  import { runningSubagents } from "./running-subagents";

  const activeSession = $derived(getActiveSession());
  const goal = $derived(activeSession?.goal ?? null);
  const todoItems = $derived(activeSession?.todos ?? []);
  const activeSubagents = $derived(
    runningSubagents(activeSession?.subagents ?? []),
  );
  const totalCount = $derived(todoItems.length);
  const completedCount = $derived(
    todoItems.filter((item) => item.status === "completed").length,
  );
  const inProgressItem = $derived(
    todoItems.find((item) => item.status === "in_progress"),
  );
  const progressPct = $derived(
    totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0,
  );
  const shouldShow = $derived(
    Boolean(goal) ||
      (totalCount > 0 && completedCount < totalCount) ||
      activeSubagents.length > 0,
  );

  let expanded = $state(false);
  let editingGoal = $state(false);
  let editGoalText = $state("");
  let pendingAction = $state<"pause" | "resume" | "edit" | "stop" | null>(null);
  let stopConfirmOpen = $state(false);
  let activeSessionId = $state<string | null>(null);
  let summaryButton = $state<HTMLButtonElement | null>(null);
  let detailsPanel = $state<HTMLDivElement | null>(null);

  function closePanel(restoreFocus = false) {
    if (!expanded) return;
    expanded = false;
    if (restoreFocus) summaryButton?.focus();
  }

  onMount(() => {
    function handlePointerDown(event: PointerEvent) {
      if (!expanded || stopConfirmOpen || !(event.target instanceof Node))
        return;
      if (
        summaryButton?.contains(event.target) ||
        detailsPanel?.contains(event.target)
      ) {
        return;
      }
      closePanel(false);
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (expanded && !stopConfirmOpen && event.key === "Escape") {
        event.preventDefault();
        closePanel(true);
      }
    }

    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  });

  $effect(() => {
    const sessionId = activeSession?.id ?? null;
    if (sessionId === activeSessionId) return;
    activeSessionId = sessionId;
    expanded = false;
    editingGoal = false;
    editGoalText = "";
    stopConfirmOpen = false;
    pendingAction = null;
    if (sessionId) void loadGoal(sessionId);
  });

  async function loadGoal(sessionId = activeSession?.id) {
    if (!sessionId) return;
    const session = activeSession;
    try {
      const result = await api.getGoal(sessionId);
      if (session?.id === sessionId) session.goal = result;
    } catch {
      if (session?.id === sessionId) session.goal = null;
    }
  }

  function statusClass(status: string): string {
    switch (status) {
      case "active":
        return "text-primary";
      case "paused":
        return "text-warning";
      case "blocked":
        return "text-error";
      case "completed":
        return "text-success";
      default:
        return "text-muted-foreground";
    }
  }

  async function runGoalAction(
    action: "pause" | "resume" | "stop",
    request: (sessionId: string) => Promise<unknown>,
  ) {
    if (!activeSession?.id || pendingAction) return;
    pendingAction = action;
    const sessionId = activeSession.id;
    try {
      await request(sessionId);
      await loadGoal(sessionId);
    } catch (error) {
      showNotification(
        `Failed to ${action} goal: ${api.errorMessage(error)}`,
        "error",
      );
    } finally {
      pendingAction = null;
    }
  }

  function startEditGoal() {
    if (!goal || pendingAction) return;
    editGoalText = goal.description;
    editingGoal = true;
  }

  function cancelEditGoal() {
    if (pendingAction === "edit") return;
    editingGoal = false;
    editGoalText = "";
  }

  async function submitEditGoal() {
    if (!activeSession?.id || pendingAction || !editGoalText.trim()) return;
    pendingAction = "edit";
    const sessionId = activeSession.id;
    try {
      await api.editGoal(sessionId, editGoalText.trim());
      editingGoal = false;
      editGoalText = "";
      await loadGoal(sessionId);
    } catch (error) {
      showNotification(
        `Failed to edit goal: ${api.errorMessage(error)}`,
        "error",
      );
    } finally {
      pendingAction = null;
    }
  }

  async function stopGoal() {
    stopConfirmOpen = false;
    await runGoalAction("stop", api.stopGoal);
  }
</script>

{#snippet progressSpinner(compact = false)}
  <span
    class="relative flex shrink-0 items-center justify-center rounded-full bg-primary/10 ring-1 ring-primary/20 {compact
      ? 'size-4'
      : 'size-5'}"
    role="status"
    aria-label="In progress"
  >
    <Loader2
      class="animate-spin text-primary {compact ? 'size-3' : 'size-3.5'}"
      strokeWidth={2.5}
    />
  </span>
{/snippet}

{#if shouldShow}
  <div class="sticky top-0 z-20 shrink-0 bg-background/95 backdrop-blur-sm">
    <div class="mx-auto w-full max-w-4xl px-4 py-2 lg:px-6">
      <section
        class="relative rounded-lg border border-border/70 bg-background shadow-sm"
        aria-labelledby="task-dock-title"
      >
        <button
          bind:this={summaryButton}
          type="button"
          onclick={() => (expanded = !expanded)}
          class="flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left transition-colors hover:bg-secondary/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
          aria-expanded={expanded}
          aria-controls="task-dock-details"
        >
          <div
            class="flex size-6 shrink-0 items-center justify-center rounded-md {goal ||
            totalCount > 0
              ? 'bg-primary/10 text-primary'
              : 'bg-info/10 text-info'}"
          >
            {#if goal || totalCount > 0}
              <Target class="size-3.5" />
            {:else}
              <Bot class="size-3.5" />
            {/if}
          </div>
          <div class="flex min-w-0 flex-1 items-center gap-2">
            <h2
              id="task-dock-title"
              class="min-w-0 truncate text-sm font-medium"
            >
              {goal?.description ??
                (activeSubagents.length > 0 ? "Running agents" : "Progress")}
            </h2>
            {#if inProgressItem}
              <span class="text-muted-foreground/50" aria-hidden="true">·</span>
              {@render progressSpinner(true)}
              <span class="min-w-0 truncate text-xs text-muted-foreground">
                {inProgressItem.content}
              </span>
            {/if}
            {#if activeSubagents.length > 0}
              <span class="text-muted-foreground/50" aria-hidden="true">·</span>
              <RunningSubagents subagents={activeSubagents} compact />
            {/if}
            {#if goal}
              <span
                class="hidden shrink-0 text-[11px] font-medium capitalize sm:inline {statusClass(
                  goal.status,
                )}">{goal.status}</span
              >
            {/if}
          </div>
          {#if totalCount > 0}
            <div class="hidden shrink-0 items-center gap-2 sm:flex">
              <div class="h-1 w-16 overflow-hidden rounded-full bg-secondary">
                <div
                  class="h-full rounded-full bg-primary transition-[width] duration-300"
                  style:width={`${progressPct}%`}
                ></div>
              </div>
              <span class="text-[11px] tabular-nums text-muted-foreground">
                {completedCount} / {totalCount}
              </span>
            </div>
            <span
              class="shrink-0 text-[11px] tabular-nums text-muted-foreground sm:hidden"
              >{completedCount}/{totalCount}</span
            >
          {/if}
          <ChevronDown
            class="size-4 shrink-0 text-muted-foreground transition-transform duration-200 {expanded
              ? 'rotate-180'
              : ''}"
          />
        </button>

        {#if expanded}
          <div
            bind:this={detailsPanel}
            id="task-dock-details"
            class="absolute inset-x-0 top-full z-30 mt-1 max-h-[min(70vh,32rem)] overflow-y-auto rounded-lg border border-border bg-background shadow-lg"
          >
            {#if goal}
              <section class="px-3 py-3" aria-labelledby="task-goal-heading">
                <div class="flex items-center justify-between gap-3">
                  <h3
                    id="task-goal-heading"
                    class="text-[11px] font-medium uppercase tracking-wide text-muted-foreground"
                  >
                    Goal
                  </h3>
                  <span
                    class="flex items-center gap-1.5 text-xs font-medium capitalize {statusClass(
                      goal.status,
                    )}"
                  >
                    {#if goal.status === "completed"}<Check
                        class="size-3.5"
                      />{:else}<span class="size-1.5 rounded-full bg-current"
                      ></span>{/if}
                    {goal.status}
                  </span>
                </div>
                {#if editingGoal}
                  <div class="mt-2">
                    <textarea
                      bind:value={editGoalText}
                      rows={3}
                      disabled={pendingAction === "edit"}
                      class="w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-sm outline-none transition-shadow focus:ring-2 focus:ring-ring disabled:opacity-50"
                      aria-label="Goal description"
                      onkeydown={(event: KeyboardEvent) => {
                        if (event.key === "Enter" && !event.shiftKey) {
                          event.preventDefault();
                          void submitEditGoal();
                        } else if (event.key === "Escape") {
                          event.stopPropagation();
                          cancelEditGoal();
                        }
                      }}
                    ></textarea>
                    <div class="mt-2 flex justify-end gap-1.5">
                      <button
                        type="button"
                        onclick={cancelEditGoal}
                        disabled={pendingAction === "edit"}
                        class="h-8 rounded-md border border-border px-3 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
                        >Cancel</button
                      >
                      <button
                        type="button"
                        onclick={submitEditGoal}
                        disabled={!editGoalText.trim() ||
                          pendingAction === "edit"}
                        class="inline-flex h-8 items-center gap-1.5 rounded-md border border-primary/30 bg-primary/10 px-3 text-xs font-medium text-primary transition-colors hover:border-primary/40 hover:bg-primary/15 disabled:opacity-50"
                      >
                        {#if pendingAction === "edit"}<Loader2
                            class="size-3.5 animate-spin"
                          />{/if} Save
                      </button>
                    </div>
                  </div>
                {:else}
                  <p class="mt-2 text-sm leading-relaxed text-foreground">
                    {goal.description}
                  </p>
                {/if}
              </section>
            {/if}

            {#if activeSubagents.length > 0}
              <RunningSubagents subagents={activeSubagents} />
            {/if}

            {#if totalCount > 0}
              <section class="border-t border-border px-3 py-2.5">
                <div class="space-y-0.5">
                  {#each todoItems as item (item.id)}
                    <div
                      class="flex items-start gap-2.5 rounded-md px-1 py-1.5"
                    >
                      {#if item.status === "completed"}
                        <CheckCircle2
                          class="mt-0.5 size-4 shrink-0 text-success"
                        />
                      {:else if item.status === "in_progress"}
                        <span class="mt-0.5">
                          {@render progressSpinner()}
                        </span>
                      {:else}
                        <Circle
                          class="mt-0.5 size-4 shrink-0 text-muted-foreground/60"
                        />
                      {/if}
                      <span
                        class="text-sm leading-relaxed {item.status ===
                        'completed'
                          ? 'text-muted-foreground line-through'
                          : item.status === 'in_progress'
                            ? 'font-medium text-foreground'
                            : 'text-muted-foreground'}">{item.content}</span
                      >
                    </div>
                  {/each}
                </div>
              </section>
            {/if}

            {#if goal && goal.status !== "completed" && !editingGoal}
              <footer
                class="flex flex-wrap items-center gap-1.5 border-t border-border bg-secondary/20 px-3 py-2"
              >
                {#if goal.status === "active"}
                  <button
                    type="button"
                    onclick={() => runGoalAction("pause", api.pauseGoal)}
                    disabled={pendingAction !== null}
                    class="inline-flex h-8 items-center gap-1.5 rounded-md border border-warning/25 bg-warning/5 px-3 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/10 disabled:opacity-50"
                  >
                    {#if pendingAction === "pause"}<Loader2
                        class="size-3.5 animate-spin"
                      />{:else}<Pause class="size-3.5" />{/if} Pause
                  </button>
                {:else if goal.status === "paused"}
                  <button
                    type="button"
                    onclick={() => runGoalAction("resume", api.resumeGoal)}
                    disabled={pendingAction !== null}
                    class="inline-flex h-8 items-center gap-1.5 rounded-md border border-success/25 bg-success/5 px-3 text-xs font-medium text-success transition-colors hover:border-success/40 hover:bg-success/10 disabled:opacity-50"
                  >
                    {#if pendingAction === "resume"}<Loader2
                        class="size-3.5 animate-spin"
                      />{:else}<Play class="size-3.5" />{/if} Resume
                  </button>
                {/if}
                <button
                  type="button"
                  onclick={startEditGoal}
                  disabled={pendingAction !== null}
                  class="inline-flex h-8 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
                  ><Pencil class="size-3.5" /> Edit</button
                >
                <button
                  type="button"
                  onclick={() => (stopConfirmOpen = true)}
                  disabled={pendingAction !== null}
                  class="ml-auto inline-flex h-8 items-center gap-1.5 rounded-md border border-destructive/25 bg-destructive/5 px-3 text-xs font-medium text-destructive transition-colors hover:border-destructive/40 hover:bg-destructive/10 disabled:opacity-50"
                >
                  {#if pendingAction === "stop"}<Loader2
                      class="size-3.5 animate-spin"
                    />{:else}<Square class="size-3.5" />{/if} Stop
                </button>
              </footer>
            {/if}
          </div>
        {/if}
      </section>
    </div>
  </div>
{/if}

<ConfirmDialog
  open={stopConfirmOpen}
  title="Stop current goal?"
  message="The autonomous goal will stop and no further steps will be started. Existing messages and completed work are preserved."
  confirmText="Stop goal"
  onConfirm={stopGoal}
  onCancel={() => (stopConfirmOpen = false)}
/>
