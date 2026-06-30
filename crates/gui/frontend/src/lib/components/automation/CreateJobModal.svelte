<script lang="ts">
  import { X, Plus, AlertCircle, CheckCircle } from "lucide-svelte";
  import {
    createCronJob,
    updateCronJob,
    createSession,
    listProjects,
    getCwd,
  } from "../../api";
  import type { ProjectInfo } from "../../api";
  import type { CronJob } from "../../automation.svelte";

  interface Props {
    editingJob?: CronJob;
    onClose: () => void;
    onSaved: () => void;
  }

  let { editingJob, onClose, onSaved }: Props = $props();

  let name = $state(editingJob?.name ?? "");
  let schedule = $state(editingJob?.schedule ?? "");
  let actionType = $state(editingJob?.action.type ?? "send_message");
  let use_new_session = $state(editingJob ? false : true);
  let session_id = $state(editingJob?.action.session_id ?? "");
  let content = $state(editingJob?.action.content ?? "");
  let command = $state(editingJob?.action.command ?? "");
  let working_dir = $state(editingJob?.action.working_dir ?? "");
  let max_runs = $state(editingJob?.max_runs ?? "");
  let expires_at = $state(
    editingJob?.expires_at
      ? utcToLocalDatetimeLocal(editingJob.expires_at)
      : "",
  );

  let projects = $state<ProjectInfo[]>([]);
  let selected_project_id = $state("");
  let showAdvanced = $state(false);
  let scheduleValid = $state<boolean | null>(null);
  let scheduleError = $state("");
  let saving = $state(false);
  let error = $state("");

  function extractErrorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    if (typeof e === "string") return e;
    if (e && typeof e === "object" && "message" in e)
      return String((e as Record<string, unknown>).message);
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }

  function validateSchedule(s: string) {
    if (!s) {
      scheduleValid = null;
      scheduleError = "";
      return;
    }
    const parts = s.trim().split(/\s+/);
    if (parts.length !== 5 && parts.length !== 6) {
      scheduleValid = false;
      scheduleError = "Cron must have 5 or 6 fields";
      return;
    }
    scheduleValid = true;
    scheduleError = "";
  }

  $effect(() => {
    validateSchedule(schedule);
  });

  // Load projects when opening new session mode
  async function loadProjects() {
    try {
      projects = await listProjects();
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }

  $effect(() => {
    if (
      use_new_session &&
      actionType === "send_message" &&
      projects.length === 0
    ) {
      loadProjects();
    }
  });

  async function save() {
    if (!name.trim() || !schedule.trim() || scheduleValid === false) return;

    saving = true;
    error = "";

    if (
      actionType === "send_message" &&
      !use_new_session &&
      !session_id.trim()
    ) {
      error = "Session ID is required for existing session";
      saving = false;
      return;
    }

    let final_session_id = session_id;

    if (actionType === "send_message" && use_new_session) {
      try {
        const project = projects.find((p) => p.id === selected_project_id);
        const workingDir = project?.dir ?? (await getCwd());
        final_session_id = await createSession(
          workingDir,
          "safe",
          selected_project_id || undefined,
        );
      } catch (e: unknown) {
        error = "Failed to create session: " + extractErrorMessage(e);
        saving = false;
        return;
      }
    }

    const action: Record<string, unknown> = { type: actionType };
    if (actionType === "send_message") {
      action.session_id = final_session_id.trim() || undefined;
      action.content = content;
    } else if (actionType === "shell") {
      action.command = command;
      action.working_dir = working_dir.trim() || undefined;
    }

    const payload: Record<string, unknown> = {
      name: name.trim(),
      schedule: schedule.trim(),
      action,
    };

    const max_runsNum = max_runs ? parseInt(String(max_runs), 10) : undefined;
    if (max_runsNum !== undefined && !Number.isNaN(max_runsNum)) {
      payload.max_runs = max_runsNum;
    }
    if (expires_at) {
      payload.expires_at = new Date(expires_at).toISOString();
    }

    try {
      if (editingJob) {
        await updateCronJob(editingJob.id, payload);
      } else {
        await createCronJob(
          payload as {
            name: string;
            schedule: string;
            action: Record<string, unknown>;
            max_runs?: number;
            expires_at?: string;
          },
        );
      }
      onSaved();
    } catch (e: unknown) {
      error = extractErrorMessage(e);
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

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4"
>
  <div
    class="bg-background rounded-xl border border-border shadow-2xl w-full max-w-lg max-h-[90vh] flex flex-col"
  >
    <!-- Header -->
    <div
      class="flex items-center justify-between px-5 py-4 border-b border-border shrink-0"
    >
      <h2 class="text-base font-semibold">
        {editingJob ? "Edit Task" : "New Scheduled Task"}
      </h2>
      <button
        type="button"
        onclick={onClose}
        class="p-1 rounded hover:bg-secondary text-muted-foreground"
      >
        <X class="w-5 h-5" />
      </button>
    </div>

    <!-- Body -->
    <div class="flex-1 overflow-y-auto px-5 py-4 space-y-4">
      {#if error}
        <div class="text-sm text-red-600 bg-red-500/10 rounded-lg px-3 py-2">
          {error}
        </div>
      {/if}

      <!-- Name -->
      <div>
        <label class="block text-sm font-medium mb-1"
          >Name <span class="text-red-500">*</span></label
        >
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
          <p class="text-xs text-muted-foreground mt-1">
            5 or 6 fields (optional seconds prefix)
          </p>
        {/if}
      </div>

      <!-- Action type -->
      <div>
        <label class="block text-sm font-medium mb-1">Action Type</label>
        <div class="flex gap-2">
          <button
            type="button"
            onclick={() => (actionType = "send_message")}
            class="flex-1 py-2 rounded-lg border text-sm transition-colors
                   {actionType === 'send_message'
              ? 'bg-primary/10 border-primary text-primary'
              : 'border-input hover:bg-muted'}"
          >
            💬 Send Message
          </button>
          <button
            type="button"
            onclick={() => (actionType = "shell")}
            class="flex-1 py-2 rounded-lg border text-sm transition-colors
                   {actionType === 'shell'
              ? 'bg-primary/10 border-primary text-primary'
              : 'border-input hover:bg-muted'}"
          >
            🔧 Shell Command
          </button>
        </div>
      </div>

      <!-- Action fields -->
      {#if actionType === "send_message"}
        <!-- Session target -->
        {#if !editingJob}
          <div>
            <label class="block text-sm font-medium mb-1">Session Target</label>
            <div class="flex gap-2">
              <button
                type="button"
                onclick={() => (use_new_session = true)}
                class="flex-1 py-2 rounded-lg border text-sm transition-colors
                       {use_new_session
                  ? 'bg-primary/10 border-primary text-primary'
                  : 'border-input hover:bg-muted'}"
              >
                New Session
              </button>
              <button
                type="button"
                onclick={() => (use_new_session = false)}
                class="flex-1 py-2 rounded-lg border text-sm transition-colors
                       {!use_new_session
                  ? 'bg-primary/10 border-primary text-primary'
                  : 'border-input hover:bg-muted'}"
              >
                Existing Session
              </button>
            </div>
          </div>
        {/if}

        {#if !use_new_session}
          <div>
            <label class="block text-sm font-medium mb-1">Session ID</label>
            <input
              type="text"
              bind:value={session_id}
              placeholder="project-alpha"
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
        {:else if !editingJob}
          <div>
            <label class="block text-sm font-medium mb-1"
              >Project (optional)</label
            >
            <select
              bind:value={selected_project_id}
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="">None</option>
              {#each projects as project (project.id)}
                <option value={project.id}>{project.name}</option>
              {/each}
            </select>
          </div>
        {/if}

        <div>
          <label class="block text-sm font-medium mb-1">Content</label>
          <textarea
            bind:value={content}
            placeholder="Review today's tasks..."
            rows="3"
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring"
          ></textarea>
          <p class="text-xs text-muted-foreground mt-1">
            Supports {"{{date}}"}, {"{{time}}"}
          </p>
        </div>
      {:else}
        <div>
          <label class="block text-sm font-medium mb-1">Command</label>
          <textarea
            bind:value={command}
            placeholder="echo hello"
            rows="3"
            lang="en"
            spellcheck={false}
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm font-mono resize-none focus:outline-none focus:ring-2 focus:ring-ring"
          ></textarea>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Working Directory</label
          >
          <input
            type="text"
            bind:value={working_dir}
            placeholder="."
            class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          />
        </div>
      {/if}

      <!-- Advanced toggle -->
      <button
        type="button"
        onclick={() => (showAdvanced = !showAdvanced)}
        class="text-sm text-muted-foreground hover:text-foreground flex items-center gap-1"
      >
        <Plus
          class="w-3.5 h-3.5 transition-transform {showAdvanced
            ? 'rotate-45'
            : ''}"
        />
        {showAdvanced ? "Hide" : "Show"} advanced options
      </button>

      {#if showAdvanced}
        <div class="space-y-3 pt-2 border-t border-border">
          <div>
            <label class="block text-sm font-medium mb-1">Max Runs</label>
            <input
              type="number"
              bind:value={max_runs}
              placeholder="Unlimited"
              min="1"
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <p class="text-xs text-muted-foreground mt-1">
              Leave empty for unlimited
            </p>
          </div>
          <div>
            <label class="block text-sm font-medium mb-1">Expires At</label>
            <input
              type="datetime-local"
              bind:value={expires_at}
              class="w-full px-3 py-2 rounded-lg border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <p class="text-xs text-muted-foreground mt-1">
              Leave empty for never
            </p>
          </div>
        </div>
      {/if}
    </div>

    <!-- Footer -->
    <div
      class="flex items-center justify-end gap-2 px-5 py-4 border-t border-border shrink-0"
    >
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
        disabled={!name.trim() ||
          !schedule.trim() ||
          scheduleValid === false ||
          saving}
        class="px-4 py-2 rounded-lg text-sm bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
      >
        {saving ? "Saving..." : editingJob ? "Save Changes" : "Create Task"}
      </button>
    </div>
  </div>
</div>
