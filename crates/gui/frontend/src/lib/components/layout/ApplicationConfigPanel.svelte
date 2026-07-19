<script lang="ts">
  import { onMount } from "svelte";
  import {
    Bell,
    Check,
    ChevronDown,
    Monitor,
    Moon,
    PanelLeft,
    Rabbit,
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
    type ActivityGroupExpansionPreference,
  } from "../../settings.svelte";
  import { showNotification } from "../../state.svelte";
  import * as api from "../../api";

  interface Props {
    onDirtyChange?: (dirty: boolean) => void;
  }

  let { onDirtyChange }: Props = $props();

  let saved = $state<GuiPreferences>(snapshotGuiPreferences());
  let draft = $state<GuiPreferences>(snapshotGuiPreferences());
  let saving = $state(false);
  let error = $state<string | null>(null);
  let pet_packs = $state<api.PetPack[]>([]);
  let pet_packs_loading = $state(true);
  let pet_packs_error = $state<string | null>(null);
  let pet_preview_changed = false;

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

  const activityGroupOptions: Array<{
    id: ActivityGroupExpansionPreference;
    label: string;
  }> = [
    { id: "collapsed", label: "Collapsed" },
    { id: "expanded", label: "Expanded" },
    { id: "latest", label: "Latest" },
    { id: "while_running", label: "While running" },
  ];

  let petSync = Promise.resolve();

  const dirty = $derived(
    JSON.stringify($state.snapshot(draft)) !==
      JSON.stringify($state.snapshot(saved)),
  );
  const selected_pet_pack_id = $derived(
    pet_packs.some((pack) => pack.id === draft.desktop_pet.selected_pet_id)
      ? draft.desktop_pet.selected_pet_id
      : (pet_packs[0]?.id ?? null),
  );

  $effect(() => {
    onDirtyChange?.(dirty);
  });

  onMount(() => {
    let disposed = false;
    void api
      .listPetPacks()
      .then((packs) => {
        if (disposed) return;
        pet_packs = packs;
        pet_packs_error = null;
      })
      .catch((load_error) => {
        if (disposed) return;
        pet_packs_error = api.errorMessage(load_error);
        console.error("Failed to list desktop pet packs:", load_error);
      })
      .finally(() => {
        if (!disposed) pet_packs_loading = false;
      });

    return () => {
      disposed = true;
      if (dirty) {
        const original = $state.snapshot(saved);
        replaceGuiPreferences(original);
        if (pet_preview_changed) {
          void syncPetPreview(original.desktop_pet).catch(() => {});
        }
      }
      onDirtyChange?.(false);
    };
  });

  function syncPetPreview(value: GuiPreferences["desktop_pet"]): Promise<void> {
    pet_preview_changed = true;
    const snapshot = {
      enabled: value.enabled,
      selected_pet_id: pet_packs.some(
        (pack) => pack.id === value.selected_pet_id,
      )
        ? value.selected_pet_id
        : (pet_packs[0]?.id ?? null),
    };
    petSync = petSync
      .catch(() => {})
      .then(async () => {
        await api.selectPetPack(snapshot.selected_pet_id);
        await api.setPetEnabled(
          snapshot.enabled && snapshot.selected_pet_id !== null,
        );
      });
    return petSync;
  }

  function preview(update: (value: GuiPreferences) => void) {
    const previous_pet = { ...draft.desktop_pet };
    update(draft);
    replaceGuiPreferences(draft);
    error = null;
    const effective_pet = {
      enabled: draft.desktop_pet.enabled,
      selected_pet_id: selected_pet_pack_id,
    };
    if (
      effective_pet.enabled !== previous_pet.enabled ||
      effective_pet.selected_pet_id !== previous_pet.selected_pet_id
    ) {
      void syncPetPreview(effective_pet).catch((syncError) => {
        error = `Desktop pet preview failed: ${api.errorMessage(syncError)}`;
      });
    }
  }

  function restore(target: GuiPreferences) {
    const copy = $state.snapshot(target);
    Object.assign(draft.appearance, copy.appearance);
    Object.assign(draft.layout, copy.layout);
    Object.assign(draft.notifications, copy.notifications);
    Object.assign(draft.desktop_pet, copy.desktop_pet);
    Object.assign(draft.chat, copy.chat);
    replaceGuiPreferences(copy);
    error = null;
    void syncPetPreview(copy.desktop_pet).catch((syncError) => {
      error = `Desktop pet preview failed: ${api.errorMessage(syncError)}`;
    });
  }

  async function save() {
    if (!dirty || saving) return;
    saving = true;
    error = null;
    try {
      // Normalize a stale persisted selection only on an explicit save and
      // only when the pack list loaded; keep the user's selection otherwise.
      if (
        !pet_packs_loading &&
        !pet_packs_error &&
        !pet_packs.some((pack) => pack.id === draft.desktop_pet.selected_pet_id)
      ) {
        draft.desktop_pet.selected_pet_id = pet_packs[0]?.id ?? null;
        if (pet_packs.length === 0) draft.desktop_pet.enabled = false;
      }
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
            <Rabbit size={15} class="text-muted-foreground" /> Desktop pet
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Keep a compact companion nearby for session status and requests.
          </p>
        </div>
        <div class="divide-y divide-border">
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Pet pack</div>
              <div class="text-xs text-muted-foreground">
                Local packs in
                <code class="rounded bg-code-bg px-1 py-0.5">~/.yomi/pets</code>
              </div>
              {#if pet_packs_loading}
                <div class="mt-1 text-xs text-muted-foreground">
                  Loading local packs…
                </div>
              {:else if pet_packs_error}
                <div class="mt-1 text-xs text-error">
                  Could not load packs: {pet_packs_error}
                </div>
              {:else if pet_packs.length === 0}
                <div class="mt-1 text-xs text-warning">
                  No local pet packs found.
                </div>
              {/if}
            </div>
            <select
              value={selected_pet_pack_id ?? ""}
              onchange={(event) =>
                preview(
                  (value) =>
                    (value.desktop_pet.selected_pet_id =
                      event.currentTarget.value || null),
                )}
              disabled={pet_packs_loading || pet_packs.length === 0}
              class="h-8 min-w-48 rounded-md border border-border bg-background px-2 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-40"
              aria-label="Desktop pet pack"
            >
              {#if draft.desktop_pet.selected_pet_id === null || !pet_packs.some((pack) => pack.id === draft.desktop_pet.selected_pet_id)}
                <option value="" disabled>Choose a pet</option>
              {/if}
              {#each pet_packs as pack (pack.id)}
                <option value={pack.id}>{pack.display_name}</option>
              {/each}
            </select>
          </div>
          <label
            class="flex items-center justify-between gap-4 px-4 py-3.5 {pet_packs.length ===
            0
              ? 'cursor-not-allowed'
              : 'cursor-pointer'}"
          >
            <div>
              <div class="text-sm text-foreground">Enable desktop pet</div>
              <div class="text-xs text-muted-foreground">
                Preview the always-on-top pet window immediately.
              </div>
            </div>
            <input
              type="checkbox"
              checked={draft.desktop_pet.enabled}
              disabled={pet_packs_loading || pet_packs.length === 0}
              onchange={(event) =>
                preview((value) => {
                  value.desktop_pet.enabled = event.currentTarget.checked;
                  if (
                    value.desktop_pet.enabled &&
                    value.desktop_pet.selected_pet_id === null
                  ) {
                    value.desktop_pet.selected_pet_id = selected_pet_pack_id;
                  }
                })}
              class="h-4 w-4 accent-primary disabled:opacity-40"
            />
          </label>
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
          <div
            class="flex flex-col gap-3 border-t border-border px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Activity details</div>
              <div class="text-xs text-muted-foreground">
                Choose which tool and thinking groups open automatically.
              </div>
            </div>
            <select
              value={draft.chat.activityGroupExpansion}
              onchange={(event) =>
                preview(
                  (value) =>
                    (value.chat.activityGroupExpansion = event.currentTarget
                      .value as ActivityGroupExpansionPreference),
                )}
              class="h-8 rounded-md border border-border bg-background px-2 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
              aria-label="Activity group expansion"
            >
              {#each activityGroupOptions as option (option.id)}
                <option value={option.id}>{option.label}</option>
              {/each}
            </select>
          </div>
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
