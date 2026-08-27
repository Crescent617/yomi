<script lang="ts">
  import {
    AlertCircle,
    CheckCircle2,
    ChevronDown,
    Clock3,
    FolderKanban,
    Loader2,
    MessageSquare,
    Terminal,
  } from "lucide-svelte";
  import {
    createCronJob,
    errorMessage,
    getCwd,
    isNeverExpires,
    listProjects,
    updateCronJob,
    type CronJob,
    type ProjectInfo,
  } from "../../api";
  import Modal from "../ui/Modal.svelte";
  import { buildCronAction } from "./cron-action";

  interface Props {
    editingJob?: CronJob;
    onClose: () => void;
    onSaved: () => void;
  }

  let { editingJob, onClose, onSaved }: Props = $props();

  // The modal is remounted on each open, so the prop intentionally only
  // initializes the editable form state.
  // svelte-ignore state_referenced_locally
  const initialEditingJob = editingJob;
  let name = $state(initialEditingJob?.name ?? "");
  let schedule = $state(initialEditingJob?.schedule ?? "");
  let actionType = $state<"send_message" | "shell">(
    initialEditingJob?.action.type === "shell" ? "shell" : "send_message",
  );
  let use_new_session = $state(!initialEditingJob);
  let session_id = $state(initialEditingJob?.action.session_id ?? "");
  let content = $state(initialEditingJob?.action.content ?? "");
  let command = $state(initialEditingJob?.action.command ?? "");
  let working_dir = $state(initialEditingJob?.action.working_dir ?? "");
  let max_runs = $state<string | number>(initialEditingJob?.max_runs || "");
  let expires_at = $state(
    initialEditingJob && !isNeverExpires(initialEditingJob.expires_at)
      ? utcToLocalDatetimeLocal(initialEditingJob.expires_at)
      : "",
  );

  let projects = $state<ProjectInfo[]>([]);
  let selected_project_id = $state("");
  let showAdvanced = $state(
    Boolean(initialEditingJob) &&
      ((initialEditingJob?.max_runs ?? 0) > 0 ||
        !isNeverExpires(initialEditingJob?.expires_at)),
  );
  let loadingProjects = $state(false);
  let saving = $state(false);
  let attempted = $state(false);
  let error = $state("");

  const scheduleError = $derived.by(() => {
    if (!schedule.trim()) return "Schedule is required";
    const fields = schedule.trim().split(/\s+/);
    if (fields.length !== 5 && fields.length !== 6) {
      return "Use 5 fields, or 6 with an optional seconds prefix";
    }
    return "";
  });

  const validationErrors = $derived.by(() => {
    const errors: Record<string, string> = {};
    if (!name.trim()) errors.name = "Task name is required";
    if (scheduleError) errors.schedule = scheduleError;

    if (actionType === "send_message") {
      if (!content.trim()) errors.content = "Message content is required";
      // 编辑模式下留空 session_id = 每次运行新建独立会话（per-run）
      if (!editingJob && !use_new_session && !session_id.trim()) {
        errors.session_id = "Session ID is required";
      }
    } else if (!command.trim()) {
      errors.command = "Command is required";
    }

    if (max_runs !== "" && max_runs != null) {
      const value = Number(max_runs);
      if (!Number.isInteger(value) || value < 1) {
        errors.max_runs = "Enter a positive whole number";
      }
    }
    if (expires_at && Number.isNaN(new Date(expires_at).getTime())) {
      errors.expires_at = "Enter a valid expiration date";
    }
    return errors;
  });

  const scheduleValid = $derived(Boolean(schedule.trim()) && !scheduleError);

  async function loadProjects() {
    if (loadingProjects || projects.length > 0) return;
    loadingProjects = true;
    try {
      projects = await listProjects();
    } catch (e) {
      console.error("Failed to load projects:", e);
    } finally {
      loadingProjects = false;
    }
  }

  $effect(() => {
    if (
      use_new_session &&
      actionType === "send_message" &&
      projects.length === 0
    ) {
      void loadProjects();
    }
  });

  function close() {
    if (!saving) onClose();
  }

  async function save(event?: SubmitEvent) {
    event?.preventDefault();
    if (saving) return;
    attempted = true;
    error = "";
    if (Object.keys(validationErrors).length > 0) return;

    saving = true;

    try {
      const project = projects.find((item) => item.id === selected_project_id);
      // cwd 只在 per-run 路径被消费；绑定时懒取，getCwd 失败不阻塞保存
      const needsCwd =
        actionType === "send_message" &&
        (use_new_session || !session_id.trim());
      const cwd = needsCwd ? await getCwd().catch(() => undefined) : undefined;
      const action = buildCronAction({
        actionType,
        content,
        command,
        shellWorkingDir: working_dir,
        useNewSession: use_new_session,
        sessionId: session_id,
        project,
        selectedProjectId: selected_project_id || undefined,
        existingTemplate: editingJob?.action.session_template,
        cwd,
      });

      const payload: Record<string, unknown> = {
        name: name.trim(),
        schedule: schedule.trim(),
        action: JSON.stringify(action),
      };
      if (max_runs !== "" && max_runs != null) {
        payload.max_runs = Number(max_runs);
      } else if ((editingJob?.max_runs ?? 0) > 0) {
        // Field cleared in edit mode: 0 is the no-limit sentinel.
        payload.max_runs = 0;
      }
      if (expires_at) {
        payload.expires_at = new Date(expires_at).toISOString();
      } else if (!isNeverExpires(editingJob?.expires_at)) {
        // Field cleared in edit mode: zero timestamp is the never-expires sentinel.
        payload.expires_at = new Date(0).toISOString();
      }

      if (editingJob) {
        await updateCronJob(editingJob.id, payload);
      } else {
        await createCronJob(
          payload as {
            name: string;
            schedule: string;
            action: string;
            max_runs?: number;
            expires_at?: string;
          },
        );
      }
      onSaved();
    } catch (e: unknown) {
      error = errorMessage(e);
    } finally {
      saving = false;
    }
  }

  function utcToLocalDatetimeLocal(iso: string): string {
    const date = new Date(iso);
    const pad = (value: number) => String(value).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }
</script>

<Modal
  open={true}
  size="lg"
  onClose={close}
  title={editingJob ? "Edit scheduled task" : "New scheduled task"}
>
  <form id="automation-task-form" onsubmit={save} class="space-y-6">
    <p class="-mt-1 text-sm text-muted-foreground">
      {editingJob
        ? "Update when this task runs and what it should do."
        : "Run a message or shell command automatically on a cron schedule."}
    </p>

    {#if error}
      <div
        class="flex items-start gap-2 rounded-md border border-error/20 bg-error/10 px-3 py-2 text-sm text-error"
        role="alert"
      >
        <AlertCircle class="mt-0.5 size-4 shrink-0" />
        <span>{error}</span>
      </div>
    {/if}

    <section aria-labelledby="task-basics-heading">
      <h3
        id="task-basics-heading"
        class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
      >
        Basics
      </h3>
      <div class="mt-3 grid gap-4 sm:grid-cols-2">
        <div>
          <label for="task-name" class="mb-1.5 block text-sm font-medium">
            Name <span class="text-error">*</span>
          </label>
          <input
            id="task-name"
            type="text"
            bind:value={name}
            placeholder="Daily standup reminder"
            aria-invalid={attempted && Boolean(validationErrors.name)}
            class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
          />
          {#if attempted && validationErrors.name}
            <p class="mt-1 text-xs text-error">{validationErrors.name}</p>
          {/if}
        </div>

        <div>
          <label for="task-schedule" class="mb-1.5 block text-sm font-medium">
            Schedule <span class="text-error">*</span>
          </label>
          <div class="relative">
            <Clock3
              class="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <input
              id="task-schedule"
              type="text"
              bind:value={schedule}
              placeholder="0 9 * * *"
              spellcheck={false}
              aria-invalid={(attempted || Boolean(schedule)) && !scheduleValid}
              class="h-9 w-full rounded-md border border-input bg-background pl-9 pr-8 font-mono text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
            />
            {#if scheduleValid}
              <CheckCircle2
                class="absolute right-3 top-1/2 size-3.5 -translate-y-1/2 text-success"
              />
            {/if}
          </div>
          <p
            class="mt-1 text-xs {(attempted || Boolean(schedule)) &&
            !scheduleValid
              ? 'text-error'
              : 'text-muted-foreground'}"
          >
            {(attempted || Boolean(schedule)) && !scheduleValid
              ? scheduleError
              : "5 fields, or 6 with an optional seconds prefix · local time"}
          </p>
        </div>
      </div>
    </section>

    <section
      class="border-t border-border pt-5"
      aria-labelledby="task-action-heading"
    >
      <h3
        id="task-action-heading"
        class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
      >
        Action
      </h3>

      <div
        class="mt-3 grid grid-cols-2 rounded-md bg-secondary p-1"
        role="group"
      >
        <button
          type="button"
          onclick={() => (actionType = "send_message")}
          aria-pressed={actionType === "send_message"}
          class="inline-flex h-8 items-center justify-center gap-2 rounded text-sm transition-colors {actionType ===
          'send_message'
            ? 'bg-background font-medium text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'}"
        >
          <MessageSquare class="size-4" /> Send message
        </button>
        <button
          type="button"
          onclick={() => (actionType = "shell")}
          aria-pressed={actionType === "shell"}
          class="inline-flex h-8 items-center justify-center gap-2 rounded text-sm transition-colors {actionType ===
          'shell'
            ? 'bg-background font-medium text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'}"
        >
          <Terminal class="size-4" /> Shell command
        </button>
      </div>

      {#if actionType === "send_message"}
        <div class="mt-4 space-y-4">
          {#if !editingJob}
            <div>
              <span class="mb-1.5 block text-sm font-medium"
                >Session target</span
              >
              <div
                class="grid grid-cols-2 rounded-md bg-secondary p-1"
                role="group"
              >
                <button
                  type="button"
                  onclick={() => (use_new_session = true)}
                  aria-pressed={use_new_session}
                  class="h-8 rounded text-sm transition-colors {use_new_session
                    ? 'bg-background font-medium text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                >
                  Fresh session per run
                </button>
                <button
                  type="button"
                  onclick={() => (use_new_session = false)}
                  aria-pressed={!use_new_session}
                  class="h-8 rounded text-sm transition-colors {!use_new_session
                    ? 'bg-background font-medium text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                >
                  Existing session
                </button>
              </div>
            </div>
          {/if}

          {#if !use_new_session}
            <div>
              <label
                for="task-session"
                class="mb-1.5 block text-sm font-medium"
              >
                Session ID {#if !editingJob}<span class="text-error">*</span
                  >{/if}
              </label>
              <input
                id="task-session"
                type="text"
                bind:value={session_id}
                placeholder={editingJob
                  ? "Leave empty for a fresh session per run"
                  : "Session ID"}
                aria-invalid={attempted && Boolean(validationErrors.session_id)}
                class="h-9 w-full rounded-md border border-input bg-background px-3 font-mono text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
              />
              {#if attempted && validationErrors.session_id}
                <p class="mt-1 text-xs text-error">
                  {validationErrors.session_id}
                </p>
              {/if}
            </div>
          {:else if !editingJob}
            <div>
              <label
                for="task-project"
                class="mb-1.5 block text-sm font-medium"
              >
                Project <span class="font-normal text-muted-foreground"
                  >optional</span
                >
              </label>
              <div class="relative">
                <FolderKanban
                  class="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
                />
                <select
                  id="task-project"
                  bind:value={selected_project_id}
                  disabled={loadingProjects}
                  class="h-9 w-full appearance-none rounded-md border border-input bg-background pl-9 pr-8 text-sm outline-none transition-shadow focus:ring-2 focus:ring-ring disabled:opacity-60"
                >
                  <option value="">
                    {loadingProjects ? "Loading projects..." : "No project"}
                  </option>
                  {#each projects as project (project.id)}
                    <option value={project.id}>{project.name}</option>
                  {/each}
                </select>
                <ChevronDown
                  class="pointer-events-none absolute right-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
                />
              </div>
            </div>
          {/if}

          <div>
            <label for="task-content" class="mb-1.5 block text-sm font-medium">
              Message <span class="text-error">*</span>
            </label>
            <textarea
              id="task-content"
              bind:value={content}
              placeholder="Review today's tasks..."
              rows="4"
              aria-invalid={attempted && Boolean(validationErrors.content)}
              class="w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
            ></textarea>
            <p
              class="mt-1 text-xs {attempted && validationErrors.content
                ? 'text-error'
                : 'text-muted-foreground'}"
            >
              {attempted && validationErrors.content
                ? validationErrors.content
                : "Supports {{date}} and {{time}} variables"}
            </p>
          </div>
        </div>
      {:else}
        <div class="mt-4 space-y-4">
          <div>
            <label for="task-command" class="mb-1.5 block text-sm font-medium">
              Command <span class="text-error">*</span>
            </label>
            <textarea
              id="task-command"
              bind:value={command}
              placeholder="cargo test"
              rows="4"
              lang="en"
              spellcheck={false}
              aria-invalid={attempted && Boolean(validationErrors.command)}
              class="w-full resize-y rounded-md border border-input bg-background px-3 py-2 font-mono text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
            ></textarea>
            {#if attempted && validationErrors.command}
              <p class="mt-1 text-xs text-error">{validationErrors.command}</p>
            {/if}
          </div>
          <div>
            <label
              for="task-working-dir"
              class="mb-1.5 block text-sm font-medium"
            >
              Working directory
              <span class="font-normal text-muted-foreground">optional</span>
            </label>
            <input
              id="task-working-dir"
              type="text"
              bind:value={working_dir}
              placeholder="Use daemon working directory"
              class="h-9 w-full rounded-md border border-input bg-background px-3 font-mono text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring"
            />
          </div>
        </div>
      {/if}
    </section>

    <section class="border-t border-border pt-4">
      <button
        type="button"
        onclick={() => (showAdvanced = !showAdvanced)}
        class="flex w-full items-center justify-between rounded-md py-1 text-left text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
        aria-expanded={showAdvanced}
      >
        <span>Advanced options</span>
        <ChevronDown
          class="size-4 transition-transform {showAdvanced ? 'rotate-180' : ''}"
        />
      </button>

      {#if showAdvanced}
        <div class="mt-3 grid gap-4 sm:grid-cols-2">
          <div>
            <label for="task-max-runs" class="mb-1.5 block text-sm font-medium">
              Max runs
            </label>
            <input
              id="task-max-runs"
              type="number"
              bind:value={max_runs}
              placeholder="Unlimited"
              min="1"
              step="1"
              aria-invalid={attempted && Boolean(validationErrors.max_runs)}
              class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
            />
            <p
              class="mt-1 text-xs {attempted && validationErrors.max_runs
                ? 'text-error'
                : 'text-muted-foreground'}"
            >
              {attempted && validationErrors.max_runs
                ? validationErrors.max_runs
                : "Leave empty for unlimited"}
            </p>
          </div>
          <div>
            <label for="task-expires" class="mb-1.5 block text-sm font-medium">
              Expires at
            </label>
            <input
              id="task-expires"
              type="datetime-local"
              bind:value={expires_at}
              aria-invalid={attempted && Boolean(validationErrors.expires_at)}
              class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm outline-none transition-shadow focus:ring-2 focus:ring-ring aria-[invalid=true]:border-error aria-[invalid=true]:ring-error/20"
            />
            <p
              class="mt-1 text-xs {attempted && validationErrors.expires_at
                ? 'text-error'
                : 'text-muted-foreground'}"
            >
              {attempted && validationErrors.expires_at
                ? validationErrors.expires_at
                : "Leave empty for no expiration"}
            </p>
          </div>
        </div>
      {/if}
    </section>
  </form>

  {#snippet footer()}
    <button
      type="button"
      onclick={close}
      disabled={saving}
      class="h-9 rounded-md border border-input px-4 text-sm text-foreground transition-colors hover:bg-secondary disabled:opacity-50"
    >
      Cancel
    </button>
    <button
      type="submit"
      form="automation-task-form"
      disabled={saving}
      class="inline-flex h-9 min-w-28 items-center justify-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
    >
      {#if saving}
        <Loader2 class="size-4 animate-spin" />
        Saving...
      {:else if editingJob}
        Save changes
      {:else}
        Create task
      {/if}
    </button>
  {/snippet}
</Modal>
