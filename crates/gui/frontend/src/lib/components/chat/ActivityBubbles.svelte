<script lang="ts">
  import {
    Bot,
    BookOpen,
    Check,
    CheckCircle2,
    Circle,
    Copy,
    ListChecks,
    Loader2,
    Pause,
    Pencil,
    Play,
    Square,
    Target,
    Terminal,
  } from "lucide-svelte";
  import { onDestroy, onMount } from "svelte";
  import { fly, scale } from "svelte/transition";
  import {
    getActiveSession,
    runningSessions,
    showNotification,
    streamingMessages,
  } from "../../state.svelte";
  import * as api from "../../api";
  import { clock } from "../../clock.svelte";
  import { elapsedLabel } from "../layout/status-activity";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import PopoverPanel from "../ui/PopoverPanel.svelte";
  import RunningSubagents from "./RunningSubagents.svelte";
  import { loadedSkills } from "./loaded-skills";
  import {
    runningSubagents,
    runningSubagentsSummary,
  } from "./running-subagents";

  const activeSession = $derived(getActiveSession());
  const goal = $derived(activeSession?.goal ?? null);
  const todoItems = $derived(activeSession?.todos ?? []);
  const activeSubagents = $derived(
    runningSubagents(activeSession?.subagents ?? []),
  );
  const activeShells = $derived(
    runningSessions.find((s) => s.id === activeSession?.id)
      ?.background_shells ?? [],
  );
  const loadedSkillList = $derived(
    loadedSkills([
      ...(activeSession?.messages ?? []),
      ...(streamingMessages[activeSession?.id ?? ""] ?? []),
    ]),
  );
  const totalCount = $derived(todoItems.length);
  const completedCount = $derived(
    todoItems.filter((item) => item.status === "completed").length,
  );
  const progressPct = $derived(
    totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0,
  );

  // One bubble per concern, each visible only while it has something to say.
  const showGoal = $derived(Boolean(goal));
  const showTodos = $derived(totalCount > 0 && completedCount < totalCount);
  const showAgents = $derived(activeSubagents.length > 0);
  const showShells = $derived(activeShells.length > 0);
  const showSkills = $derived(loadedSkillList.length > 0);
  const showAny = $derived(
    showGoal || showTodos || showAgents || showShells || showSkills,
  );

  type BubbleKind = "goal" | "todos" | "agents" | "shells" | "skills";
  let expanded = $state<BubbleKind | null>(null);

  // Progress ring geometry (r=6 in a 16×16 viewBox).
  const RING_C = 2 * Math.PI * 6;

  // Close a panel whose bubble lost its reason to exist (e.g. the last todo
  // completed while its list was open).
  $effect(() => {
    if (expanded === "goal" && !showGoal) expanded = null;
    if (expanded === "todos" && !showTodos) expanded = null;
    if (expanded === "agents" && !showAgents) expanded = null;
    if (expanded === "shells" && !showShells) expanded = null;
    if (expanded === "skills" && !showSkills) expanded = null;
  });

  function toggle(kind: BubbleKind) {
    expanded = expanded === kind ? null : kind;
  }

  // ── Goal state / actions (ported from the old TaskDock) ────────────────
  let editingGoal = $state(false);
  let editGoalText = $state("");
  let pendingAction = $state<"pause" | "resume" | "edit" | "stop" | null>(null);
  let stopConfirmOpen = $state(false);
  let activeSessionId = $state<string | null>(null);
  let containerRef = $state<HTMLDivElement | null>(null);

  function closePanel(restoreFocus = false) {
    if (!expanded) return;
    const kind = expanded;
    expanded = null;
    if (restoreFocus) {
      containerRef
        ?.querySelector<HTMLButtonElement>(`button[data-bubble='${kind}']`)
        ?.focus();
    }
  }

  onMount(() => {
    function handlePointerDown(event: PointerEvent) {
      if (!expanded || stopConfirmOpen || !(event.target instanceof Node))
        return;
      if (containerRef?.contains(event.target)) return;
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
    expanded = null;
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

  // ── Background shell log-path copy ──────────────────────────────────────
  let copiedShellPath = $state<string | null>(null);
  let copyResetTimer: ReturnType<typeof setTimeout> | undefined;

  onDestroy(() => {
    if (copyResetTimer) clearTimeout(copyResetTimer);
  });

  async function copyShellLogPath(outputPath: string) {
    try {
      await navigator.clipboard.writeText(outputPath);
      copiedShellPath = outputPath;
      if (copyResetTimer) clearTimeout(copyResetTimer);
      copyResetTimer = setTimeout(() => (copiedShellPath = null), 1500);
    } catch (error) {
      showNotification(
        `Failed to copy shell log path: ${api.errorMessage(error)}`,
        "error",
      );
    }
  }

  const bubbleClass =
    "group flex items-center rounded-l-full border border-r-0 border-border bg-background/95 py-1.5 pl-2.5 pr-2 shadow-md backdrop-blur transition-colors hover:bg-secondary/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
  const panelClass =
    "absolute right-full top-0 z-30 mr-2 w-80 max-w-[calc(100vw-4rem)] origin-top-right";
</script>

{#snippet progressSpinner()}
  <span
    class="relative flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/10 ring-1 ring-primary/20"
    role="status"
    aria-label="In progress"
  >
    <Loader2 class="size-3.5 animate-spin text-primary" strokeWidth={2.5} />
  </span>
{/snippet}

{#if showAny}
  <!-- Right-edge activity bubbles: goal / todos / agents, docked to the
       chat's right gutter so the message column keeps its full width.
       Top-anchored — the vertical center belongs to the query navigator. -->
  <div
    bind:this={containerRef}
    class="absolute right-2 top-3 z-20 flex flex-col items-end gap-2"
  >
    {#if showGoal && goal}
      <div class="relative" transition:fly={{ x: 14, duration: 140 }}>
        <button
          type="button"
          class={bubbleClass}
          data-bubble="goal"
          onclick={() => toggle("goal")}
          aria-expanded={expanded === "goal"}
          aria-label="Goal details, status {goal.status}"
          title="Goal ({goal.status}): {goal.description}"
        >
          <Target
            class="size-3.5 shrink-0 {statusClass(goal.status)}"
            aria-hidden="true"
          />
        </button>
        {#if expanded === "goal"}
          <div
            class={panelClass}
            transition:scale={{ start: 0.9, duration: 130 }}
          >
            <PopoverPanel
              title="Goal"
              padded
              bodyClass="max-h-[min(60vh,28rem)] overflow-y-auto"
            >
              {#snippet headerActions()}
                <span
                  class="flex items-center gap-1.5 text-[10px] font-medium capitalize {statusClass(
                    goal.status,
                  )}"
                >
                  {#if goal.status === "completed"}
                    <Check class="size-3" />
                  {:else}
                    <span class="size-1.5 rounded-full bg-current"></span>
                  {/if}
                  {goal.status}
                </span>
              {/snippet}

              {#if editingGoal}
                <div>
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
                      {#if pendingAction === "edit"}
                        <Loader2 class="size-3.5 animate-spin" />
                      {/if}
                      Save
                    </button>
                  </div>
                </div>
              {:else}
                <p class="text-sm leading-relaxed text-foreground">
                  {goal.description}
                </p>
              {/if}

              {#if goal.status !== "completed" && !editingGoal}
                <div
                  class="mt-2.5 flex flex-wrap items-center gap-1.5 border-t border-border pt-2.5"
                >
                  {#if goal.status === "active"}
                    <button
                      type="button"
                      onclick={() => runGoalAction("pause", api.pauseGoal)}
                      disabled={pendingAction !== null}
                      class="inline-flex h-8 items-center gap-1.5 rounded-md border border-warning/25 bg-warning/5 px-3 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/10 disabled:opacity-50"
                    >
                      {#if pendingAction === "pause"}
                        <Loader2 class="size-3.5 animate-spin" />
                      {:else}
                        <Pause class="size-3.5" />
                      {/if}
                      Pause
                    </button>
                  {:else if goal.status === "paused"}
                    <button
                      type="button"
                      onclick={() => runGoalAction("resume", api.resumeGoal)}
                      disabled={pendingAction !== null}
                      class="inline-flex h-8 items-center gap-1.5 rounded-md border border-success/25 bg-success/5 px-3 text-xs font-medium text-success transition-colors hover:border-success/40 hover:bg-success/10 disabled:opacity-50"
                    >
                      {#if pendingAction === "resume"}
                        <Loader2 class="size-3.5 animate-spin" />
                      {:else}
                        <Play class="size-3.5" />
                      {/if}
                      Resume
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
                    {#if pendingAction === "stop"}
                      <Loader2 class="size-3.5 animate-spin" />
                    {:else}
                      <Square class="size-3.5" />
                    {/if}
                    Stop
                  </button>
                </div>
              {/if}
            </PopoverPanel>
          </div>
        {/if}
      </div>
    {/if}

    {#if showTodos}
      <div class="relative" transition:fly={{ x: 14, duration: 140 }}>
        <button
          type="button"
          class="{bubbleClass} gap-1.5"
          data-bubble="todos"
          onclick={() => toggle("todos")}
          aria-expanded={expanded === "todos"}
          aria-label="Task progress details"
          title="{completedCount} of {totalCount} tasks done"
        >
          <svg viewBox="0 0 16 16" class="size-4 shrink-0" aria-hidden="true">
            <circle
              cx="8"
              cy="8"
              r="6"
              fill="none"
              stroke-width="2.5"
              class="stroke-secondary"
            />
            <circle
              cx="8"
              cy="8"
              r="6"
              fill="none"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-dasharray={RING_C}
              stroke-dashoffset={RING_C * (1 - progressPct / 100)}
              transform="rotate(-90 8 8)"
              class="stroke-primary transition-[stroke-dashoffset] duration-300"
            />
          </svg>
          <span class="text-[11px] tabular-nums text-muted-foreground">
            {completedCount}/{totalCount}
          </span>
        </button>
        {#if expanded === "todos"}
          <div
            class={panelClass}
            transition:scale={{ start: 0.9, duration: 130 }}
          >
            <PopoverPanel title="Tasks">
              {#snippet headerActions()}
                <span
                  class="inline-flex items-center gap-1 text-[10px] tabular-nums text-muted-foreground"
                >
                  <ListChecks class="size-3" />
                  {completedCount}/{totalCount}
                </span>
              {/snippet}
              <div class="space-y-0.5 px-2 py-1.5">
                {#each todoItems as item (item.id)}
                  <div class="flex items-start gap-2.5 rounded-md px-1 py-1.5">
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
            </PopoverPanel>
          </div>
        {/if}
      </div>
    {/if}

    {#if showAgents}
      <div class="relative" transition:fly={{ x: 14, duration: 140 }}>
        <button
          type="button"
          class="{bubbleClass} gap-1"
          data-bubble="agents"
          onclick={() => toggle("agents")}
          aria-expanded={expanded === "agents"}
          aria-label="Running agents details"
          title={runningSubagentsSummary(activeSubagents)}
        >
          <Bot class="size-3.5 shrink-0 text-info" aria-hidden="true" />
          <span class="text-[10px] tabular-nums text-muted-foreground">
            {activeSubagents.length}
          </span>
        </button>
        {#if expanded === "agents"}
          <div
            class={panelClass}
            transition:scale={{ start: 0.9, duration: 130 }}
          >
            <PopoverPanel title="Running agents">
              {#snippet headerActions()}
                <span class="text-[10px] tabular-nums text-muted-foreground">
                  {activeSubagents.length}
                </span>
              {/snippet}
              <RunningSubagents subagents={activeSubagents} />
            </PopoverPanel>
          </div>
        {/if}
      </div>
    {/if}

    {#if showShells}
      <div class="relative" transition:fly={{ x: 14, duration: 140 }}>
        <button
          type="button"
          class="{bubbleClass} gap-1"
          data-bubble="shells"
          onclick={() => toggle("shells")}
          aria-expanded={expanded === "shells"}
          aria-label="Background shells details"
          title="{activeShells.length} background {activeShells.length > 1
            ? 'shells'
            : 'shell'} running"
        >
          <Terminal class="size-3.5 shrink-0 text-info" aria-hidden="true" />
          <span class="text-[10px] tabular-nums text-muted-foreground">
            {activeShells.length}
          </span>
        </button>
        {#if expanded === "shells"}
          <div
            class={panelClass}
            transition:scale={{ start: 0.9, duration: 130 }}
          >
            <PopoverPanel title="Background shells">
              {#snippet headerActions()}
                <span class="text-[10px] tabular-nums text-muted-foreground">
                  {activeShells.length}
                </span>
              {/snippet}
              <div class="py-1">
                {#each activeShells as shell (shell.task_id)}
                  <div
                    class="popover-list-item flex w-full items-start gap-2 px-3 py-2"
                  >
                    <Terminal class="mt-0.5 h-3.5 w-3.5 shrink-0 text-info" />
                    <span class="min-w-0 flex-1">
                      <span
                        class="block truncate font-mono text-xs font-medium"
                        title={shell.command}
                      >
                        {shell.command}
                      </span>
                      <span
                        class="block truncate text-[10px] text-muted-foreground"
                      >
                        PID {shell.pid} · {elapsedLabel(
                          shell.started_at,
                          clock.now,
                        )}
                      </span>
                    </span>
                    <button
                      type="button"
                      class="inline-flex h-6 shrink-0 items-center gap-1 rounded px-1.5 text-[10px] text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
                      onclick={() => void copyShellLogPath(shell.output_path)}
                      title={`Copy log path: ${shell.output_path}`}
                      aria-label={`Copy log path for ${shell.task_id}`}
                    >
                      {#if copiedShellPath === shell.output_path}
                        <Check class="h-3 w-3 text-success" />
                        <span>Copied</span>
                      {:else}
                        <Copy class="h-3 w-3" />
                        <span>Log</span>
                      {/if}
                    </button>
                    <span
                      class="mt-2 h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-info"
                      aria-hidden="true"
                    ></span>
                  </div>
                {/each}
              </div>
            </PopoverPanel>
          </div>
        {/if}
      </div>
    {/if}

    {#if showSkills}
      <div class="relative" transition:fly={{ x: 14, duration: 140 }}>
        <button
          type="button"
          class="{bubbleClass} gap-1"
          data-bubble="skills"
          onclick={() => toggle("skills")}
          aria-expanded={expanded === "skills"}
          aria-label="Loaded skills details"
          title="{loadedSkillList.length} skill{loadedSkillList.length > 1
            ? 's'
            : ''} read this session (may have left the context since)"
        >
          <BookOpen class="size-3.5 shrink-0 text-info" aria-hidden="true" />
          <span class="text-[10px] tabular-nums text-muted-foreground">
            {loadedSkillList.length}
          </span>
        </button>
        {#if expanded === "skills"}
          <div
            class={panelClass}
            transition:scale={{ start: 0.9, duration: 130 }}
          >
            <PopoverPanel title="Skills loaded">
              {#snippet headerActions()}
                <span class="text-[10px] tabular-nums text-muted-foreground">
                  {loadedSkillList.length}
                </span>
              {/snippet}
              <div class="py-1">
                {#each loadedSkillList as skill (skill.name)}
                  <div
                    class="flex items-start gap-2 px-3 py-2"
                    title={skill.path}
                  >
                    <BookOpen class="mt-0.5 h-3.5 w-3.5 shrink-0 text-info" />
                    <span class="min-w-0 flex-1">
                      <span
                        class="block truncate font-mono text-xs font-medium"
                      >
                        {skill.name}
                      </span>
                      <span
                        class="block truncate text-[10px] text-muted-foreground"
                      >
                        {skill.path}
                      </span>
                    </span>
                  </div>
                {/each}
              </div>
            </PopoverPanel>
          </div>
        {/if}
      </div>
    {/if}
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
