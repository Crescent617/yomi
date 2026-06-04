<script lang="ts">
  import { X, Plus, AlertCircle, CheckCircle } from "lucide-svelte";
  import { createCronJob, updateCronJob } from "../../api";
  import type { CronJob } from "../../automation.svelte";

  interface Props {
    editingJob?: CronJob;
    onClose: () => void;
    onSaved: () => void;
  }

  let { editingJob, onClose, onSaved }: Props = $props();

  let name = $state(editingJob?.name ?? "");
  let schedule = $state(editingJob?.schedule ?? "");
  let actionType = $state(editingJob?.action.ty ?? "send_message");
  let sessionId = $state(editingJob?.action.session_id ?? "");
  let content = $state(editingJob?.action.content ?? "");
  let command = $state(editingJob?.action.command ?? "");
  let workingDir = $state(editingJob?.action.working_dir ?? "");
  let maxRuns = $state(editingJob?.max_runs ?? "");
  let expiresAt = $state(editingJob?.expires_at ? utcToLocalDatetimeLocal(editingJob.expires_at) : "");

  let showAdvanced = $state(false);
  let scheduleValid = $state<boolean | null>(null);
  let scheduleError = $state("");
  let saving = $state(false);
  let error = $state("");

  function validateSchedule(s: string) {
    if (!s) { scheduleValid = null; scheduleError = ""; return; }
    // Basic cron validation: must have 5-6 fields or be a simple expression
    const parts = s.trim().split(/\s+/);
    if (parts.length !== 5 && parts.length !== 6) {
      scheduleValid = false;
      scheduleError = "Cron must have 5 or 6 fields";
      return;
    }
    scheduleValid = true;
    scheduleError = "";
  }

  $effect(() => { validateSchedule(schedule); });

  async function save() {
    if (!name.trim() || !schedule.trim() || scheduleValid === false) return;

    saving = true;
    error = "";

    const action: Record<string, unknown> = { ty: actionType };
    if (actionType === "send_message") {
      action.session_id = sessionId.trim() || undefined;
      action.content = content;
    } else if (actionType === "shell") {
      action.command = command;
      action.working_dir = workingDir.trim() || undefined;
    }

    const payload: Record<string, unknown> = {
      name: name.trim(),
      schedule: schedule.trim(),
      action,
    };

    const maxRunsNum = maxRuns ? parseInt(String(maxRuns), 10) : undefined;
    if (maxRunsNum !== undefined && !Number.isNaN(maxRunsNum)) {
      payload.max_runs = maxRunsNum;
    }
    if (expiresAt) {
      payload.expires_at = new Date(expiresAt).toISOString();
    }

    try {
      if (editingJob) {
        await updateCronJob(editingJob.id, payload);
      } else {
        await createCronJob(payload as { name: string; schedule: string; action: Record<string, unknown>; max_runs?: number; expires_at?: string });
      }
      onSaved();
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  function utcToLocalDatetimeLocal(iso: string): string {
    const d = new Date(iso);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4">
  <div class="bg-background rounded-xl border border-border shadow-2xl w-full max-w-lg max-h-[90vh] flex flex-col">
    <!-- Header -->
    <div class="flex items-center justify-between px-5 py-4 border-b border-border shrink-0">
      <h2 class="text-base font-semibold">{editingJob ? "Edit Task" : "New Scheduled Task"}</h2>
      <button type="button" onclick={onClose} class="p-1 rounded hover:bg-secondary text-muted-foreground">
        <X class="w-5 h-5" />
      </button>
    </div>

    <!-- Body -->
    <div class="flex-1 overflow-y-auto px-5 py-4 space-y-4">
      {#if error}
        <div class="text-sm text-red-600 bg-red-500/10 rounded-lg px-3 py-2">{error}</div>
      {/if}

      <!-- Name -->
      <div>
        <label class="block text-sm font-medium mb-1">Name <span class="text-red-500">*</span></label>
        <input
          type="text"
          bind:value={name}
          placeholder="Daily standup reminder"
          class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
        />
      </div>

      <!-- Schedule -->
      <div>
        <label class="block text-sm font-medium mb-1">
          Schedule <span class="text-red-500">*</span>
          {#if scheduleValid === true}
            <CheckCircle class="inline w-3.5 h-3.5 text-green-500 ml-1" />
          {:else if scheduleValid === false}
            <AlertCircle class="inline w-3.5 h-3.5 text-red-500 ml-1" />
          {/if}
        </label>
        <input
          type="text"
          bind:value={schedule}
          placeholder="0 9 * * *  (9:00 AM daily)"
          class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm font-mono focus:outline-none focus:ring-2 focus:ring-ring
                 {scheduleValid === false ? 'border-red-500' : ''}"
        />
        {#if scheduleError}
          <p class="text-xs text-red-500 mt-1">{scheduleError}</p>
        {:else}
          <p class="text-xs text-muted-foreground mt-1">5 fields (min hour dom mon dow) or 6 (with seconds)</p>
        {/if}
      </div>

      <!-- Action type -->
      <div>
        <label class="block text-sm font-medium mb-1">Action Type</label>
        <div class="flex gap-2">
          <button
            type="button"
            onclick={() => actionType = "send_message"}
            class="flex-1 py-2 rounded-lg border text-sm transition-colors
                   {actionType === 'send_message' ? 'bg-primary/10 border-primary text-primary' : 'border-input hover:bg-muted'}"
          >
            💬 Send Message
          </button>
          <button
            type="button"
            onclick={() => actionType = "shell"}
            class="flex-1 py-2 rounded-lg border text-sm transition-colors
                   {actionType === 'shell' ? 'bg-primary/10 border-primary text-primary' : 'border-input hover:bg-muted'}"
          >
            🔧 Shell Command
          </button>
        </div>
      </div>

      <!-- Action fields -->
      {#if actionType === "send_message"}
        <div>
          <label class="block text-sm font-medium mb-1">Session ID</label>
          <input
            type="text"
            bind:value={sessionId}
            placeholder="project-alpha"
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          />
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Content</label>
          <textarea
            bind:value={content}
            placeholder="Review today's tasks..."
            rows="3"
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring"
          ></textarea>
          <p class="text-xs text-muted-foreground mt-1">Supports {'{{date}}'}, {'{{time}}'}</p>
        </div>
      {:else}
        <div>
          <label class="block text-sm font-medium mb-1">Command</label>
          <textarea
            bind:value={command}
            placeholder="echo hello"
            rows="3"
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm font-mono resize-none focus:outline-none focus:ring-2 focus:ring-ring"
          ></textarea>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Working Directory</label>
          <input
            type="text"
            bind:value={workingDir}
            placeholder="."
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          />
        </div>
      {/if}

      <!-- Advanced toggle -->
      <button
        type="button"
        onclick={() => showAdvanced = !showAdvanced}
        class="text-sm text-muted-foreground hover:text-foreground flex items-center gap-1"
      >
        <Plus class="w-3.5 h-3.5 transition-transform {showAdvanced ? 'rotate-45' : ''}" />
        {showAdvanced ? "Hide" : "Show"} advanced options
      </button>

      {#if showAdvanced}
        <div class="space-y-3 pt-2 border-t border-border">
          <div>
            <label class="block text-sm font-medium mb-1">Max Runs</label>
            <input
              type="number"
              bind:value={maxRuns}
              placeholder="Unlimited"
              min="1"
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <p class="text-xs text-muted-foreground mt-1">Leave empty for unlimited</p>
          </div>
          <div>
            <label class="block text-sm font-medium mb-1">Expires At</label>
            <input
              type="datetime-local"
              bind:value={expiresAt}
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <p class="text-xs text-muted-foreground mt-1">Leave empty for never</p>
          </div>
        </div>
      {/if}
    </div>

    <!-- Footer -->
    <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-border shrink-0">
      <button
        type="button"
        onclick={onClose}
        class="px-4 py-2 rounded-lg text-sm border border-input hover:bg-secondary transition-colors"
      >
        Cancel
      </button>
      <button
        type="button"
        onclick={save}
        disabled={!name.trim() || !schedule.trim() || scheduleValid === false || saving}
        class="px-4 py-2 rounded-lg text-sm bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
      >
        {saving ? "Saving..." : editingJob ? "Save Changes" : "Create Task"}
      </button>
    </div>
  </div>
</div>
