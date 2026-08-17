<script lang="ts">
  import { onMount } from "svelte";
  import {
    AlertTriangle,
    ArrowLeft,
    CalendarClock,
    CheckCircle2,
    ChevronRight,
    Clock3,
    FileClock,
    MessageSquare,
    Pause,
    Pencil,
    Play,
    Plus,
    RefreshCw,
    RotateCcw,
    Terminal,
    Trash2,
  } from "lucide-svelte";
  import { automationStore } from "../../automation.svelte";
  import { isNeverExpires, type CronJob } from "../../api";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import IconButton from "../ui/IconButton.svelte";
  import CreateJobModal from "./CreateJobModal.svelte";
  import PanelHeader from "../layout/PanelHeader.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  let pendingAction = $state<{
    job_id: string;
    type: "run" | "toggle" | "delete";
  } | null>(null);
  let deleteTarget = $state<CronJob | null>(null);

  onMount(() => {
    void automationStore.load();
  });

  function formatDate(iso: string | null): string {
    if (!iso) return "Not yet";
    return new Date(iso).toLocaleString([], {
      dateStyle: "medium",
      timeStyle: "short",
    });
  }

  function timeUntil(iso: string | null): string {
    if (!iso) return "Not scheduled";
    const diff = new Date(iso).getTime() - Date.now();
    if (diff <= 0) return "Due now";
    const minutes = Math.floor(diff / 60_000);
    if (minutes < 60) return `in ${Math.max(1, minutes)}m`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `in ${hours}h`;
    return `in ${Math.floor(hours / 24)}d`;
  }

  function statusDotClass(status: CronJob["status"]): string {
    switch (status) {
      case "active":
        return "bg-success";
      case "paused":
        return "bg-warning";
      case "completed":
        return "bg-info";
      case "failed":
        return "bg-error";
    }
  }

  function statusTextClass(status: CronJob["status"]): string {
    switch (status) {
      case "active":
        return "text-success";
      case "paused":
        return "text-warning";
      case "completed":
        return "text-info";
      case "failed":
        return "text-error";
    }
  }

  function actionLabel(job: CronJob): string {
    return job.action.type === "shell" ? "Shell command" : "Send message";
  }

  async function runJob(job: CronJob) {
    if (pendingAction) return;
    pendingAction = { job_id: job.id, type: "run" };
    try {
      await automationStore.trigger(job.id);
    } finally {
      pendingAction = null;
    }
  }

  async function toggleJob(job: CronJob) {
    if (pendingAction) return;
    pendingAction = { job_id: job.id, type: "toggle" };
    try {
      await automationStore.toggleStatus(job);
    } finally {
      pendingAction = null;
    }
  }

  async function deleteJob() {
    if (!deleteTarget || pendingAction) return;
    const job = deleteTarget;
    pendingAction = { job_id: job.id, type: "delete" };
    try {
      await automationStore.delete(job.id);
      deleteTarget = null;
    } finally {
      pendingAction = null;
    }
  }

  function isPending(job_id: string, type: "run" | "toggle" | "delete") {
    return pendingAction?.job_id === job_id && pendingAction.type === type;
  }
</script>

<div class="flex h-full w-full flex-col">
  <PanelHeader title="Automation" icon={CalendarClock} {onToggleLeftPanel}>
    {#snippet meta()}
      <span class="hidden text-xs text-muted-foreground sm:inline"
        >{automationStore.jobs.length} tasks</span
      >
    {/snippet}
    {#snippet actions()}
      <IconButton
        label="Refresh tasks"
        icon={RefreshCw}
        spinning={automationStore.loading}
        disabled={automationStore.loading}
        onclick={() => automationStore.load()}
      />
      <button
        type="button"
        onclick={() => automationStore.openCreate()}
        class="inline-flex h-8 items-center gap-1.5 rounded-md border border-primary/30 bg-primary/10 px-3 text-xs font-medium text-primary transition-colors hover:border-primary/40 hover:bg-primary/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <Plus class="size-4" />
        New Task
      </button>
    {/snippet}
  </PanelHeader>

  <div class="flex min-h-0 flex-1 overflow-hidden">
    <aside
      class="{automationStore.selectedJob
        ? 'hidden lg:flex'
        : 'flex'} w-full shrink-0 flex-col overflow-hidden border-r border-border lg:w-80 xl:w-96"
      aria-label="Scheduled tasks"
    >
      <div
        class="flex h-10 shrink-0 items-center justify-between border-b border-border px-4 text-xs text-muted-foreground"
      >
        <span class="font-normal uppercase tracking-wide text-muted-foreground">Tasks</span>
        <span>{automationStore.jobs.length}</span>
      </div>

      {#if automationStore.loading && automationStore.jobs.length === 0}
        <div
          class="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground"
        >
          <RefreshCw class="size-4 animate-spin" />
          Loading tasks
        </div>
      {:else if automationStore.jobs.length === 0}
        <div
          class="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center"
        >
          <div
            class="flex size-11 items-center justify-center rounded-full bg-secondary text-muted-foreground"
          >
            <FileClock class="size-5" />
          </div>
          <div>
            <p class="text-sm font-medium">No scheduled tasks</p>
            <p class="mt-1 max-w-56 text-xs text-muted-foreground">
              Create a task to send a message or run a command automatically.
            </p>
          </div>
          <button
            type="button"
            onclick={() => automationStore.openCreate()}
            class="text-xs font-medium text-primary hover:underline"
          >
            Create your first task
          </button>
        </div>
      {:else}
        <div class="flex-1 overflow-y-auto">
          {#each automationStore.jobs as job (job.id)}
            <button
              type="button"
              onclick={() => automationStore.select(job.id)}
              class="group relative w-full border-b border-border/50 px-4 py-3 text-left transition-colors hover:bg-secondary/40 focus-visible:z-10 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring {automationStore.selectedJobId ===
              job.id
                ? 'bg-primary/5'
                : ''}"
            >
              {#if automationStore.selectedJobId === job.id}
                <span
                  class="absolute inset-y-2 left-0 w-0.5 rounded-r bg-primary"
                  aria-hidden="true"
                ></span>
              {/if}
              <div class="flex items-start gap-2.5">
                <span
                  class="mt-1.5 size-2 shrink-0 rounded-full {statusDotClass(
                    job.status,
                  )}"
                  title={job.status}
                ></span>
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <span class="min-w-0 flex-1 truncate text-sm font-normal"
                      >{job.name}</span
                    >
                    <ChevronRight
                      class="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5"
                    />
                  </div>
                  <div
                    class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
                  >
                    {#if job.action.type === "shell"}
                      <Terminal class="size-3 shrink-0" />
                    {:else}
                      <MessageSquare class="size-3 shrink-0" />
                    {/if}
                    <span class="truncate">{actionLabel(job)}</span>
                  </div>
                  <div
                    class="mt-1.5 flex items-center gap-2 text-[11px] text-muted-foreground"
                  >
                    <code class="truncate font-mono text-foreground/70"
                      >{job.schedule}</code
                    >
                    <span aria-hidden="true">·</span>
                    <span class="shrink-0"
                      >{job.status === "active"
                        ? timeUntil(job.next_run_at)
                        : job.status}</span
                    >
                  </div>
                </div>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </aside>

    {#if automationStore.selectedJob}
      {@const job = automationStore.selectedJob}
      <main class="flex min-w-0 flex-1 flex-col overflow-hidden">
        <div
          class="flex shrink-0 items-start justify-between gap-3 border-b border-border px-4 py-3 lg:px-6"
        >
          <div class="flex min-w-0 items-start gap-2">
            <button
              type="button"
              onclick={() => automationStore.select(null)}
              class="mt-0.5 inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary hover:text-foreground lg:hidden"
              title="Back to tasks"
              aria-label="Back to tasks"
            >
              <ArrowLeft class="size-4" />
            </button>
            <div class="min-w-0">
              <div class="flex min-w-0 items-center gap-2">
                <h2 class="truncate text-base font-semibold">{job.name}</h2>
                <span
                  class="shrink-0 text-xs font-medium capitalize {statusTextClass(
                    job.status,
                  )}">{job.status}</span
                >
              </div>
              <div
                class="mt-1 flex items-center gap-2 text-xs text-muted-foreground"
              >
                <code class="font-mono">{job.schedule}</code>
                {#if job.status === "active"}
                  <span aria-hidden="true">·</span>
                  <span>{timeUntil(job.next_run_at)}</span>
                {/if}
              </div>
            </div>
          </div>

          <div class="flex shrink-0 items-center gap-1">
            <IconButton
              label="Run now"
              icon={isPending(job.id, "run") ? RefreshCw : Play}
              spinning={isPending(job.id, "run")}
              tone="primary"
              disabled={pendingAction !== null}
              onclick={() => runJob(job)}
            />
            <IconButton
              label={job.status === "active" ? "Pause task" : "Activate task"}
              icon={isPending(job.id, "toggle")
                ? RefreshCw
                : job.status === "active"
                  ? Pause
                  : RotateCcw}
              spinning={isPending(job.id, "toggle")}
              disabled={pendingAction !== null}
              onclick={() => toggleJob(job)}
            />
            <IconButton
              label="Edit task"
              icon={Pencil}
              disabled={pendingAction !== null}
              onclick={() => automationStore.openEdit(job.id)}
            />
            <IconButton
              label="Delete task"
              icon={Trash2}
              tone="destructive"
              disabled={pendingAction !== null}
              onclick={() => (deleteTarget = job)}
            />
          </div>
        </div>

        <div class="flex-1 overflow-y-auto">
          <section class="border-b border-border px-4 py-5 lg:px-6">
            <h3
              class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >
              Action
            </h3>
            <div class="mt-3 flex items-center gap-2 text-sm font-medium">
              {#if job.action.type === "shell"}
                <Terminal class="size-4 text-primary" />
              {:else}
                <MessageSquare class="size-4 text-primary" />
              {/if}
              {actionLabel(job)}
            </div>

            {#if job.action.session_id}
              <div class="mt-2 text-xs text-muted-foreground">
                Session
                <code class="ml-1 font-mono text-foreground"
                  >{job.action.session_id}</code
                >
              </div>
            {/if}
            {#if job.action.working_dir}
              <div class="mt-2 text-xs text-muted-foreground">
                Working directory
                <code class="ml-1 font-mono text-foreground"
                  >{job.action.working_dir}</code
                >
              </div>
            {/if}
            {#if job.action.content || job.action.command}
              <pre
                class="mt-3 max-h-64 overflow-auto whitespace-pre-wrap rounded-md bg-code-bg px-3 py-2 font-mono text-xs leading-relaxed text-foreground">{job
                  .action.content ?? job.action.command}</pre>
            {/if}
          </section>

          <section class="border-b border-border px-4 py-5 lg:px-6">
            <h3
              class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >
              Schedule
            </h3>
            <dl
              class="mt-3 grid grid-cols-1 gap-x-8 gap-y-4 sm:grid-cols-2 xl:grid-cols-4"
            >
              <div>
                <dt
                  class="flex items-center gap-1.5 text-xs text-muted-foreground"
                >
                  <Clock3 class="size-3.5" /> Next run
                </dt>
                <dd class="mt-1 text-sm font-medium">
                  {formatDate(job.next_run_at)}
                </dd>
              </div>
              <div>
                <dt
                  class="flex items-center gap-1.5 text-xs text-muted-foreground"
                >
                  <CheckCircle2 class="size-3.5" /> Last run
                </dt>
                <dd class="mt-1 text-sm font-medium">
                  {formatDate(job.last_run_at)}
                </dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Runs</dt>
                <dd class="mt-1 text-sm font-medium tabular-nums">
                  {job.run_count}{#if job.max_runs}
                    / {job.max_runs}{/if}
                </dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Expires</dt>
                <dd class="mt-1 text-sm font-medium">
                  {isNeverExpires(job.expires_at)
                    ? "Never"
                    : formatDate(job.expires_at)}
                </dd>
              </div>
            </dl>
          </section>

          {#if job.last_error}
            <section class="px-4 py-5 lg:px-6">
              <h3
                class="text-xs font-medium uppercase tracking-wide text-error"
              >
                Last error
              </h3>
              <div
                class="mt-3 flex items-start gap-2 rounded-md border border-error/20 bg-error/10 px-3 py-2 text-sm text-error"
              >
                <AlertTriangle class="mt-0.5 size-4 shrink-0" />
                <span class="whitespace-pre-wrap">{job.last_error}</span>
              </div>
            </section>
          {/if}
        </div>
      </main>
    {:else}
      <main
        class="hidden min-w-0 flex-1 items-center justify-center p-8 text-center lg:flex"
      >
        <div class="max-w-64">
          <div
            class="mx-auto flex size-11 items-center justify-center rounded-full bg-secondary text-muted-foreground"
          >
            <FileClock class="size-5" />
          </div>
          <p class="mt-3 text-sm font-medium">Select a task</p>
          <p class="mt-1 text-xs text-muted-foreground">
            Choose a scheduled task to inspect its action, schedule, and run
            history.
          </p>
        </div>
      </main>
    {/if}
  </div>

  {#if automationStore.error}
    <div
      class="flex shrink-0 items-center gap-2 border-t border-error/20 bg-error/10 px-4 py-2 text-sm text-error"
      role="alert"
    >
      <AlertTriangle class="size-4 shrink-0" />
      <span class="min-w-0 flex-1 truncate">{automationStore.error}</span>
      <button
        type="button"
        onclick={() => (automationStore.error = null)}
        class="text-xs font-medium hover:underline">Dismiss</button
      >
    </div>
  {/if}
</div>

{#if automationStore.showCreateModal}
  <CreateJobModal
    editingJob={automationStore.editingJobId
      ? automationStore.selectedJob
      : undefined}
    onClose={() => automationStore.closeModal()}
    onSaved={() => {
      automationStore.closeModal();
      void automationStore.load();
    }}
  />
{/if}

<ConfirmDialog
  open={deleteTarget !== null}
  title="Delete scheduled task?"
  message={deleteTarget
    ? `“${deleteTarget.name}” will be permanently deleted. This cannot be undone.`
    : ""}
  confirmText={deleteTarget && isPending(deleteTarget.id, "delete")
    ? "Deleting..."
    : "Delete"}
  cancelText="Cancel"
  onConfirm={deleteJob}
  onCancel={() => {
    if (!pendingAction) deleteTarget = null;
  }}
/>
