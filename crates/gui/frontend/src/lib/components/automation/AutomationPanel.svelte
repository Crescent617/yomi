<script lang="ts">
  import { onMount } from "svelte";
  import { Timer, Plus, Pause, Trash2, ChevronRight, AlertTriangle, CheckCircle2, XCircle, Zap, RefreshCw } from "lucide-svelte";
  import { automationStore } from "../../automation.svelte";
  import CreateJobModal from "./CreateJobModal.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  onMount(() => {
    automationStore.load();
  });

  function formatDate(iso: string | null): string {
    if (!iso) return "—";
    const d = new Date(iso);
    return d.toLocaleString();
  }

  function timeUntil(iso: string | null): string {
    if (!iso) return "—";
    const diff = new Date(iso).getTime() - Date.now();
    if (diff <= 0) return "Due now";
    const mins = Math.floor(diff / 60000);
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    return `${Math.floor(hours / 24)}d`;
  }

  function statusIcon(status: string) {
    switch (status) {
      case "active": return CheckCircle2;
      case "paused": return Pause;
      case "completed": return CheckCircle2;
      case "error": return XCircle;
      default: return AlertTriangle;
    }
  }

  function statusColor(status: string): string {
    switch (status) {
      case "active": return "text-green-600";
      case "paused": return "text-yellow-500";
      case "completed": return "text-blue-500";
      case "error": return "text-red-500";
      default: return "text-muted-foreground";
    }
  }
</script>

<div class="flex flex-col h-full w-full">
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
    <div class="flex items-center gap-2">
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={onToggleLeftPanel}
          class="lg:hidden p-1.5 rounded hover:bg-secondary text-muted-foreground"
          title="Toggle sidebar"
        >
          <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12h18M3 6h18M3 18h18"/></svg>
        </button>
      {/if}
      <Timer class="w-5 h-5 text-primary" />
      <h1 class="text-lg font-semibold">Automation</h1>
    </div>
    <button
      type="button"
      onclick={() => automationStore.openCreate()}
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-primary text-primary-foreground text-sm hover:bg-primary/90"
    >
      <Plus class="w-4 h-4" />
      New Task
    </button>
  </div>

  <!-- Daemon status banner (placeholder) -->
  <div class="px-4 py-2 bg-muted/50 border-b border-border text-xs text-muted-foreground shrink-0">
    <span class="inline-block w-2 h-2 rounded-full bg-green-500 mr-1.5"></span>
    Daemon running — tasks will execute on schedule
  </div>

  <!-- Content -->
  <div class="flex-1 flex min-h-0 overflow-hidden">
    <!-- Job list -->
    <div class="w-full lg:w-80 xl:w-96 border-r border-border flex flex-col min-h-0 shrink-0 overflow-hidden">
      {#if automationStore.loading && automationStore.jobs.length === 0}
        <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">Loading...</div>
      {:else if automationStore.jobs.length === 0}
        <div class="flex-1 flex flex-col items-center justify-center text-muted-foreground gap-3 p-6">
          <Timer class="w-10 h-10 opacity-40" />
          <p class="text-sm">No scheduled tasks yet.</p>
          <button
            type="button"
            onclick={() => automationStore.openCreate()}
            class="text-sm text-primary hover:underline"
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
              class="w-full text-left px-4 py-3 border-b border-border/50 transition-colors
                     {automationStore.selectedJobId === job.id ? 'bg-accent' : 'hover:bg-muted/50'}"
            >
              <div class="flex items-center justify-between gap-2">
                <div class="flex items-center gap-2 min-w-0">
                  <svelte:component this={statusIcon(job.status)} class="w-4 h-4 shrink-0 {statusColor(job.status)}" />
                  <span class="font-medium truncate">{job.name}</span>
                </div>
                <ChevronRight class="w-4 h-4 shrink-0 text-muted-foreground" />
              </div>
              <div class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                <code class="font-mono bg-black/5 dark:bg-white/5 rounded px-1">{job.schedule}</code>
                <span>·</span>
                <span>{timeUntil(job.nextRunAt)}</span>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Detail panel -->
    {#if automationStore.selectedJob}
      {@const job = automationStore.selectedJob}
      <div class="hidden lg:flex flex-col flex-1 min-h-0 overflow-hidden">
        <div class="px-6 py-4 border-b border-border shrink-0">
          <div class="flex items-center justify-between">
            <h2 class="text-base font-semibold">{job.name}</h2>
            <div class="flex items-center gap-1">
              <button
                type="button"
                onclick={() => automationStore.trigger(job.id)}
                class="p-2 rounded hover:bg-secondary text-muted-foreground"
                title="Run now"
              >
                <Zap class="w-4 h-4" />
              </button>
              <button
                type="button"
                onclick={() => automationStore.toggleStatus(job)}
                class="p-2 rounded hover:bg-secondary text-muted-foreground"
                title={job.status === "active" ? "Pause" : "Activate"}
              >
                {#if job.status === "active"}
                  <Pause class="w-4 h-4" />
                {:else}
                  <RefreshCw class="w-4 h-4" />
                {/if}
              </button>
              <button
                type="button"
                onclick={() => automationStore.openEdit(job.id)}
                class="p-2 rounded hover:bg-secondary text-muted-foreground"
                title="Edit"
              >
                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              </button>
              <button
                type="button"
                onclick={() => {
                  if (confirm("Delete this task?")) automationStore.delete(job.id);
                }}
                class="p-2 rounded hover:bg-secondary text-red-500"
                title="Delete"
              >
                <Trash2 class="w-4 h-4" />
              </button>
            </div>
          </div>
          <div class="mt-1 flex items-center gap-2 text-sm text-muted-foreground">
            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-secondary text-xs capitalize">{job.status}</span>
            <code class="font-mono text-xs bg-black/5 dark:bg-white/5 rounded px-1">{job.schedule}</code>
          </div>
        </div>

        <div class="flex-1 overflow-y-auto px-6 py-4 space-y-4">
          <!-- Action -->
          <section>
            <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground mb-2">Action</h3>
            <div class="bg-muted/30 rounded-lg p-3 text-sm space-y-1">
              {#if job.action.ty === "sendMessage"}
                <div class="flex items-center gap-2">
                  <span class="text-base">💬</span>
                  <span class="font-medium">Send Message</span>
                </div>
                {#if job.action.sessionId}
                  <div class="text-muted-foreground">Session: <span class="font-mono">{job.action.sessionId}</span></div>
                {/if}
                {#if job.action.content}
                  <pre class="mt-1 bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap text-xs">{job.action.content}</pre>
                {/if}
              {:else if job.action.ty === "shell"}
                <div class="flex items-center gap-2">
                  <span class="text-base">🔧</span>
                  <span class="font-medium">Shell Command</span>
                </div>
                {#if job.action.command}
                  <pre class="mt-1 bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap text-xs">{job.action.command}</pre>
                {/if}
                {#if job.action.workingDir}
                  <div class="text-muted-foreground text-xs">Working dir: <span class="font-mono">{job.action.workingDir}</span></div>
                {/if}
              {:else}
                <div>Unknown action type: {job.action.ty}</div>
              {/if}
            </div>
          </section>

          <!-- Schedule info -->
          <section>
            <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground mb-2">Schedule</h3>
            <div class="grid grid-cols-2 gap-3 text-sm">
              <div class="bg-muted/30 rounded-lg p-3">
                <div class="text-muted-foreground text-xs mb-1">Next run</div>
                <div class="font-medium">{formatDate(job.nextRunAt)}</div>
              </div>
              <div class="bg-muted/30 rounded-lg p-3">
                <div class="text-muted-foreground text-xs mb-1">Last run</div>
                <div class="font-medium">{formatDate(job.lastRunAt)}</div>
              </div>
              <div class="bg-muted/30 rounded-lg p-3">
                <div class="text-muted-foreground text-xs mb-1">Runs</div>
                <div class="font-medium">{job.runCount}{#if job.maxRuns} / {job.maxRuns}{/if}</div>
              </div>
              <div class="bg-muted/30 rounded-lg p-3">
                <div class="text-muted-foreground text-xs mb-1">Expires</div>
                <div class="font-medium">{formatDate(job.expiresAt)}</div>
              </div>
            </div>
          </section>

          <!-- Last error -->
          {#if job.lastError}
            <section>
              <h3 class="text-xs font-semibold uppercase tracking-wider text-red-500 mb-2">Last Error</h3>
              <div class="bg-red-500/5 border border-red-500/20 rounded-lg p-3 text-sm text-red-600">{job.lastError}</div>
            </section>
          {/if}
        </div>
      </div>
    {/if}
  </div>

  {#if automationStore.error}
    <div class="shrink-0 px-4 py-2 bg-red-500/10 border-t border-red-500/20 text-sm text-red-600 flex items-center gap-2">
      <AlertTriangle class="w-4 h-4" />
      {automationStore.error}
      <button type="button" onclick={() => automationStore.error = null} class="ml-auto text-xs hover:underline">Dismiss</button>
    </div>
  {/if}
</div>

{#if automationStore.showCreateModal}
  <CreateJobModal
    editingJob={automationStore.editingJobId ? automationStore.selectedJob : undefined}
    onClose={() => automationStore.closeModal()}
    onSaved={() => { automationStore.closeModal(); automationStore.load(); }}
  />
{/if}
