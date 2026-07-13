<script lang="ts">
  import { onMount } from "svelte";
  import {
    Bell,
    Check,
    ChevronDown,
    Monitor,
    Moon,
    PanelLeft,
    RotateCcw,
    Save,
    Sun,
    Type,
  } from "lucide-svelte";
  import {
    defaultGuiPreferences,
    replaceGuiPreferences,
    saveGuiPreferences,
    snapshotGuiPreferences,
    type FontSizePreference,
    type GuiPreferences,
    type ThemePreference,
  } from "../../settings.svelte";
  import { showNotification } from "../../state.svelte";

  interface Props {
    onDirtyChange?: (dirty: boolean) => void;
  }

  let { onDirtyChange }: Props = $props();

  let saved = $state<GuiPreferences>(snapshotGuiPreferences());
  let draft = $state<GuiPreferences>(snapshotGuiPreferences());
  let saving = $state(false);
  let error = $state<string | null>(null);

  const themes: Array<{
    id: ThemePreference;
    label: string;
    icon: typeof Sun;
  }> = [
    { id: "light", label: "Light", icon: Sun },
    { id: "dark", label: "Dark", icon: Moon },
    { id: "system", label: "System", icon: Monitor },
  ];

  const fontSizes: Array<{ id: FontSizePreference; label: string }> = [
    { id: "xs", label: "Compact" },
    { id: "sm", label: "Small" },
    { id: "base", label: "Medium" },
    { id: "lg", label: "Large" },
    { id: "xl", label: "Extra large" },
  ];

  const dirty = $derived(
    JSON.stringify($state.snapshot(draft)) !==
      JSON.stringify($state.snapshot(saved)),
  );

  $effect(() => {
    onDirtyChange?.(dirty);
  });

  onMount(() => {
    return () => {
      if (dirty) replaceGuiPreferences(saved);
      onDirtyChange?.(false);
    };
  });

  function preview(update: (value: GuiPreferences) => void) {
    update(draft);
    replaceGuiPreferences(draft);
    error = null;
  }

  function restore(target: GuiPreferences) {
    const copy = $state.snapshot(target);
    Object.assign(draft.appearance, copy.appearance);
    Object.assign(draft.layout, copy.layout);
    Object.assign(draft.notifications, copy.notifications);
    Object.assign(draft.chat, copy.chat);
    replaceGuiPreferences(copy);
    error = null;
  }

  async function save() {
    if (!dirty || saving) return;
    saving = true;
    error = null;
    try {
      await saveGuiPreferences($state.snapshot(draft));
      saved = snapshotGuiPreferences();
      restore(saved);
      showNotification("Application preferences saved", "success");
    } catch (saveError) {
      console.error("Failed to save application preferences:", saveError);
      error =
        saveError instanceof Error ? saveError.message : String(saveError);
      showNotification("Failed to save application preferences", "error");
    } finally {
      saving = false;
    }
  }

  function cancel() {
    restore(saved);
  }

  function reset() {
    restore(defaultGuiPreferences);
  }
</script>

<div class="min-h-0 min-w-0 flex-1 overflow-y-auto bg-background">
  <div class="w-full px-4 py-5 sm:px-6 lg:py-7">
    <div class="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div>
        <div class="flex items-center gap-2">
          <h2 class="text-base font-semibold text-foreground">Application</h2>
          {#if dirty}
            <span
              class="rounded-full bg-warning/10 px-2 py-0.5 text-[11px] font-medium text-warning"
              >Unsaved</span
            >
          {:else}
            <span
              class="inline-flex items-center gap-1 text-[11px] text-muted-foreground"
            >
              <Check size={12} /> Saved locally
            </span>
          {/if}
        </div>
        <p class="mt-1 text-sm text-muted-foreground">
          Personalize Yomi on this device. Changes preview immediately.
        </p>
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          onclick={reset}
          class="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        >
          <RotateCcw size={13} /> Reset
        </button>
        <button
          type="button"
          onclick={cancel}
          disabled={!dirty || saving}
          class="rounded-md border border-border px-2.5 py-1.5 text-xs text-foreground transition-colors hover:bg-secondary disabled:opacity-40"
        >
          Cancel
        </button>
        <button
          type="button"
          onclick={save}
          disabled={!dirty || saving}
          class="inline-flex items-center gap-1.5 rounded-md border border-primary/30 bg-primary/10 px-3 py-1.5 text-xs font-medium text-primary transition-colors hover:bg-primary/15 disabled:opacity-40"
        >
          <Save size={13} />
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>

    {#if error}
      <div
        class="mb-4 rounded-lg border border-error/30 bg-error/10 px-3 py-2 text-xs text-error"
      >
        {error}
      </div>
    {/if}

    <div class="space-y-4">
      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <Type size={15} class="text-muted-foreground" /> Appearance
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Choose how Yomi looks and scales.
          </p>
        </div>
        <div class="divide-y divide-border">
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Theme</div>
              <div class="text-xs text-muted-foreground">
                Follow your system or choose a fixed appearance.
              </div>
            </div>
            <div class="inline-flex w-fit rounded-lg bg-secondary/70 p-1">
              {#each themes as theme (theme.id)}
                <button
                  type="button"
                  onclick={() =>
                    preview((value) => (value.appearance.theme = theme.id))}
                  class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs transition-colors {draft
                    .appearance.theme === theme.id
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                >
                  <theme.icon size={13} />
                  {theme.label}
                </button>
              {/each}
            </div>
          </div>
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Interface size</div>
              <div class="text-xs text-muted-foreground">
                Scale text and controls across the application.
              </div>
            </div>
            <div class="inline-flex w-fit rounded-lg bg-secondary/70 p-1">
              {#each fontSizes as size (size.id)}
                <button
                  type="button"
                  onclick={() =>
                    preview((value) => (value.appearance.fontSize = size.id))}
                  class="rounded-md px-2.5 py-1.5 text-xs transition-colors {draft
                    .appearance.fontSize === size.id
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                >
                  {size.label}
                </button>
              {/each}
            </div>
          </div>
        </div>
      </section>

      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <PanelLeft size={15} class="text-muted-foreground" /> Layout
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Restore your workspace the way you left it.
          </p>
        </div>
        <div class="divide-y divide-border">
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Sidebar width</div>
              <div class="text-xs text-muted-foreground">
                Set the default width for the project sidebar.
              </div>
            </div>
            <div class="flex min-w-52 items-center gap-3">
              <input
                type="range"
                min="160"
                max="400"
                step="8"
                value={draft.layout.sidebarWidth}
                oninput={(event) =>
                  preview(
                    (value) =>
                      (value.layout.sidebarWidth = Number(
                        event.currentTarget.value,
                      )),
                  )}
                class="w-36 accent-primary"
              />
              <span
                class="w-12 text-right text-xs tabular-nums text-muted-foreground"
                >{draft.layout.sidebarWidth}px</span
              >
            </div>
          </div>
        </div>
      </section>

      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <ChevronDown size={15} class="text-muted-foreground" /> Chat
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Defaults for new conversations.
          </p>
        </div>
        <div>
          <label
            class="flex cursor-pointer items-center justify-between gap-4 px-4 py-3.5"
          >
            <div>
              <div class="text-sm text-foreground">Follow new messages</div>
              <div class="text-xs text-muted-foreground">
                Keep the latest streaming response in view.
              </div>
            </div>
            <input
              type="checkbox"
              checked={draft.chat.autoScroll}
              onchange={(event) =>
                preview(
                  (value) =>
                    (value.chat.autoScroll = event.currentTarget.checked),
                )}
              class="h-4 w-4 accent-primary"
            />
          </label>
        </div>
      </section>

      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <Bell size={15} class="text-muted-foreground" /> Notifications
          </div>
        </div>
        <label
          class="flex cursor-pointer items-center justify-between gap-4 px-4 py-3.5"
        >
          <div>
            <div class="text-sm text-foreground">Notifications</div>
            <div class="text-xs text-muted-foreground">
              Show notification messages inside Yomi. Kernel events stay
              connected.
            </div>
          </div>
          <input
            type="checkbox"
            checked={draft.notifications.enabled}
            onchange={(event) =>
              preview(
                (value) =>
                  (value.notifications.enabled = event.currentTarget.checked),
              )}
            class="h-4 w-4 accent-primary"
          />
        </label>
      </section>
    </div>
  </div>
</div>
