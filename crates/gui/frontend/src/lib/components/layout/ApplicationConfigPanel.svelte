<script lang="ts">
  import { onMount } from "svelte";
  import {
    Bell,
    Info,
    MessageSquare,
    Monitor,
    Moon,
    PanelLeft,
    Rabbit,
    Sun,
    Type,
    Zap,
  } from "lucide-svelte";
  import {
    guiPreferences,
    applyGuiPreferences,
    scheduleGuiPreferencesSave,
    type FontSizePreference,
    type GuiPreferences,
    type ThemePreference,
    type ActivityGroupExpansionPreference,
  } from "../../settings.svelte";
  import * as api from "../../api";

  let error = $state<string | null>(null);
  let pet_packs = $state<api.PetPack[]>([]);
  let pet_packs_loading = $state(true);
  let pet_packs_error = $state<string | null>(null);

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

  const pet_scale_options: Array<{ value: number; label: string }> = [
    { value: 0.5, label: "50%" },
    { value: 0.75, label: "75%" },
    { value: 1, label: "100%" },
    { value: 1.5, label: "150%" },
    { value: 2, label: "200%" },
  ];

  let petSync = Promise.resolve();
  let keepAwakeSync = Promise.resolve();

  const selected_pet_pack_id = $derived(
    pet_packs.some(
      (pack) => pack.id === guiPreferences.desktop_pet.selected_pet_id,
    )
      ? guiPreferences.desktop_pet.selected_pet_id
      : (pet_packs[0]?.id ?? null),
  );

  onMount(() => {
    let disposed = false;
    void api
      .listPetPacks()
      .then((packs) => {
        if (disposed) return;
        pet_packs = packs;
        pet_packs_error = null;
        normalizePetSelection();
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
    };
  });

  function syncPetPreview(value: GuiPreferences["desktop_pet"]): Promise<void> {
    const snapshot = {
      enabled: value.enabled,
      selected_pet_id: pet_packs.some(
        (pack) => pack.id === value.selected_pet_id,
      )
        ? value.selected_pet_id
        : (pet_packs[0]?.id ?? null),
      scale: value.scale,
    };
    petSync = petSync
      .catch(() => {})
      .then(async () => {
        await api.selectPetPack(snapshot.selected_pet_id);
        await api.setPetScale(snapshot.scale);
        await api.setPetEnabled(
          snapshot.enabled && snapshot.selected_pet_id !== null,
        );
      });
    return petSync;
  }

  function syncKeepAwake(enabled: boolean): Promise<void> {
    keepAwakeSync = keepAwakeSync
      .catch(() => {})
      .then(() => api.setKeepAwake(enabled).then(() => undefined));
    return keepAwakeSync;
  }

  /**
   * Single write path for every control: mutate the live preferences, apply
   * visual side effects, preview OS/window side effects, and persist
   * (debounced) — no draft, no save button.
   */
  function update(mutate: (value: GuiPreferences) => void) {
    const previous_pet = { ...guiPreferences.desktop_pet };
    const previous_keep_awake = guiPreferences.power.keep_awake;
    mutate(guiPreferences);
    applyGuiPreferences(guiPreferences);
    scheduleGuiPreferencesSave();
    error = null;
    const pet = guiPreferences.desktop_pet;
    if (
      pet.enabled !== previous_pet.enabled ||
      pet.selected_pet_id !== previous_pet.selected_pet_id ||
      pet.scale !== previous_pet.scale
    ) {
      void syncPetPreview({
        enabled: pet.enabled,
        selected_pet_id: selected_pet_pack_id,
        scale: pet.scale,
      }).catch((syncError) => {
        error = `Desktop pet preview failed: ${api.errorMessage(syncError)}`;
      });
    }
    if (guiPreferences.power.keep_awake !== previous_keep_awake) {
      void syncKeepAwake(guiPreferences.power.keep_awake).catch((syncError) => {
        error = `Keep-awake preview failed: ${api.errorMessage(syncError)}`;
      });
    }
  }

  /** Persist a valid pet selection once the local pack list is known. */
  function normalizePetSelection() {
    const pet = guiPreferences.desktop_pet;
    if (pet_packs.some((pack) => pack.id === pet.selected_pet_id)) return;
    if (pet_packs.length === 0) {
      if (pet.selected_pet_id === null && !pet.enabled) return;
      update((value) => {
        value.desktop_pet.selected_pet_id = null;
        value.desktop_pet.enabled = false;
      });
    } else {
      update((value) => {
        value.desktop_pet.selected_pet_id = pet_packs[0].id;
      });
    }
  }
</script>

<div class="min-h-0 min-w-0 flex-1 overflow-y-auto bg-background">
  <div class="w-full px-4 py-5 sm:px-6 lg:py-7">
    <div class="mb-5">
      <h2 class="text-base font-semibold text-foreground">Application</h2>
      <p class="mt-1 text-sm text-muted-foreground">
        Personalize Yomi on this device. Changes apply and save automatically.
      </p>
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
                    update((value) => (value.appearance.theme = theme.id))}
                  class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs transition-colors {guiPreferences
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
                    update((value) => (value.appearance.fontSize = size.id))}
                  class="rounded-md px-2.5 py-1.5 text-xs transition-colors {guiPreferences
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
            Control how sessions appear in the sidebar.
          </p>
        </div>
        <div class="divide-y divide-border">
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Session list</div>
              <div class="text-xs text-muted-foreground">
                Show all sessions or only sessions assigned to a project.
              </div>
            </div>
            <label
              class="inline-flex cursor-pointer items-center gap-2 text-xs text-muted-foreground"
            >
              <input
                type="checkbox"
                checked={guiPreferences.layout.show_project_sessions_only}
                onchange={(event) =>
                  update(
                    (value) =>
                      (value.layout.show_project_sessions_only =
                        event.currentTarget.checked),
                  )}
                class="h-4 w-4 accent-primary"
              />
              Only project sessions
            </label>
          </div>
        </div>
      </section>

      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <Rabbit size={15} class="text-muted-foreground" /> Desktop pet
            <div class="group relative ml-0.5 inline-flex">
              <Info
                size={13}
                class="cursor-help text-muted-foreground transition-colors group-hover:text-foreground"
              />
              <div
                class="invisible absolute left-1/2 top-full z-50 w-60 -translate-x-1/2 pt-1.5 opacity-0 transition-opacity group-hover:visible group-hover:opacity-100"
              >
                <div
                  class="rounded-md border border-border bg-popover px-2.5 py-2 text-xs text-popover-foreground shadow-sm"
                >
                  Download pet packs from
                  <a
                    href="https://codex-pets.net"
                    target="_blank"
                    rel="noopener noreferrer"
                    class="underline transition-colors hover:text-primary"
                  >
                    codex-pets.net</a
                  >
                  and place them in
                  <code class="rounded bg-code-bg px-1 py-0.5"
                    >~/.yomi/pets</code
                  >.
                </div>
              </div>
            </div>
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Keep a compact companion nearby for session status.
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
                update(
                  (value) =>
                    (value.desktop_pet.selected_pet_id =
                      event.currentTarget.value || null),
                )}
              disabled={pet_packs_loading || pet_packs.length === 0}
              class="h-8 min-w-48 rounded-md border border-border bg-background px-2 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-40"
              aria-label="Desktop pet pack"
            >
              {#if guiPreferences.desktop_pet.selected_pet_id === null || !pet_packs.some((pack) => pack.id === guiPreferences.desktop_pet.selected_pet_id)}
                <option value="" disabled>Choose a pet</option>
              {/if}
              {#each pet_packs as pack (pack.id)}
                <option value={pack.id}>{pack.display_name}</option>
              {/each}
            </select>
          </div>
          <div
            class="flex flex-col gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between"
          >
            <div>
              <div class="text-sm text-foreground">Pet size</div>
              <div class="text-xs text-muted-foreground">
                Scale the desktop pet window.
              </div>
            </div>
            <select
              value={String(guiPreferences.desktop_pet.scale)}
              onchange={(event) =>
                update(
                  (value) =>
                    (value.desktop_pet.scale = Number(
                      event.currentTarget.value,
                    )),
                )}
              disabled={pet_packs_loading || pet_packs.length === 0}
              class="h-8 min-w-48 rounded-md border border-border bg-background px-2 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-40"
              aria-label="Desktop pet size"
            >
              {#each pet_scale_options as option (option.value)}
                <option value={String(option.value)}>{option.label}</option>
              {/each}
              {#if !pet_scale_options.some((option) => option.value === guiPreferences.desktop_pet.scale)}
                <option value={String(guiPreferences.desktop_pet.scale)}>
                  {Math.round(guiPreferences.desktop_pet.scale * 100)}%
                </option>
              {/if}
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
                Show the always-on-top pet window on your desktop.
              </div>
            </div>
            <input
              type="checkbox"
              checked={guiPreferences.desktop_pet.enabled}
              disabled={pet_packs_loading || pet_packs.length === 0}
              onchange={(event) =>
                update((value) => {
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
            <MessageSquare size={15} class="text-muted-foreground" /> Chat
          </div>
          <p class="mt-0.5 text-xs text-muted-foreground">
            How conversations look and behave.
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
              checked={guiPreferences.chat.autoScroll}
              onchange={(event) =>
                update(
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
              value={guiPreferences.chat.activityGroupExpansion}
              onchange={(event) =>
                update(
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
            <div class="text-sm text-foreground">In-app notifications</div>
            <div class="text-xs text-muted-foreground">
              Show notification messages inside Yomi. Kernel events stay
              connected.
            </div>
          </div>
          <input
            type="checkbox"
            checked={guiPreferences.notifications.enabled}
            onchange={(event) =>
              update(
                (value) =>
                  (value.notifications.enabled = event.currentTarget.checked),
              )}
            class="h-4 w-4 accent-primary"
          />
        </label>
      </section>

      <section
        class="overflow-hidden rounded-xl border border-border bg-card/45"
      >
        <div class="border-b border-border px-4 py-3">
          <div class="flex items-center gap-2 text-sm font-medium">
            <Zap size={15} class="text-muted-foreground" /> Power
          </div>
        </div>
        <label
          class="flex cursor-pointer items-center justify-between gap-4 px-4 py-3.5"
        >
          <div>
            <div class="text-sm text-foreground">Keep awake</div>
            <div class="text-xs text-muted-foreground">
              Prevent this device from sleeping while Yomi is running. The
              display can still turn off.
            </div>
          </div>
          <input
            type="checkbox"
            checked={guiPreferences.power.keep_awake}
            onchange={(event) =>
              update(
                (value) =>
                  (value.power.keep_awake = event.currentTarget.checked),
              )}
            class="h-4 w-4 accent-primary"
          />
        </label>
      </section>
    </div>
  </div>
</div>
