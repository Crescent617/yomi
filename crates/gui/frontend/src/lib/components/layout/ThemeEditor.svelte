<script lang="ts">
  import { untrack } from "svelte";
  import { Moon, Sun } from "lucide-svelte";
  import Modal from "../ui/Modal.svelte";
  import {
    PALETTE_GROUPS,
    parseThemeJson,
    type ColorTheme,
  } from "../../themes/palettes";
  import { applyColorTheme } from "../../themes/themes.svelte";

  interface Props {
    /** The theme to edit; cloned internally so the original stays intact. */
    theme: ColorTheme;
    isNew: boolean;
    onSave: (theme: ColorTheme) => void;
    onClose: () => void;
  }

  let { theme, isNew, onSave, onClose }: Props = $props();

  const variants = ["light", "dark"] as const;

  // One-time deep copy at mount; $state.snapshot also unwraps the parent's
  // $state proxy (structuredClone throws on proxies in WKWebView).
  let draft = $state<ColorTheme>(untrack(() => $state.snapshot(theme)));
  let variant = $state<(typeof variants)[number]>(
    document.documentElement.classList.contains("dark") ? "dark" : "light",
  );
  let jsonText = $state(
    untrack(() =>
      JSON.stringify(
        { name: draft.name, light: draft.light, dark: draft.dark },
        null,
        2,
      ),
    ),
  );
  let jsonError = $state<string | null>(null);

  // Live-apply the edited palette (mount + every draft/variant change).
  // Never mirrored to localStorage; closing the editor restores the real theme.
  $effect(() => {
    applyColorTheme(draft, variant === "dark", false);
  });

  function handleJsonInput(e: Event) {
    jsonText = (e.currentTarget as HTMLTextAreaElement).value;
    const result = parseThemeJson(jsonText);
    if (result.ok) {
      jsonError = null;
      draft.name = result.name;
      draft.light = result.light;
      draft.dark = result.dark;
    } else {
      // Keep the last valid draft; preview stays put until the JSON parses.
      jsonError = result.error;
    }
  }

  function handleSave() {
    const name = draft.name.trim();
    if (!name || jsonError !== null) return;
    onSave({ ...draft, name });
  }

  const saveDisabled = $derived(
    draft.name.trim().length === 0 || jsonError !== null,
  );
</script>

<Modal
  open={true}
  size="xl"
  fitContent
  title={isNew ? "New custom theme" : `Edit ${theme.name}`}
  {onClose}
  onSubmit={handleSave}
>
  <div class="flex h-full min-h-0 flex-col gap-3 px-5 py-4">
    <div class="flex shrink-0 flex-wrap items-center gap-3">
      <div class="inline-flex rounded-lg bg-secondary/70 p-1">
        {#each variants as v (v)}
          <button
            type="button"
            onclick={() => (variant = v)}
            class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs transition-colors {variant ===
            v
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground'}"
          >
            {#if v === "light"}<Sun size={13} />{:else}<Moon size={13} />{/if}
            {v === "light" ? "Light" : "Dark"}
          </button>
        {/each}
      </div>
      <span class="text-[11px] text-muted-foreground">
        Previewing the {variant} palette live — closing restores the active theme.
      </span>
    </div>

    <div class="flex min-h-0 flex-1 gap-4">
      <textarea
        value={jsonText}
        oninput={handleJsonInput}
        spellcheck="false"
        class="min-h-0 flex-1 resize-none overflow-y-auto rounded-md border border-input bg-code-bg px-3 py-2 font-mono text-xs leading-relaxed text-foreground focus:outline-none focus:ring-1 focus:ring-ring"
      ></textarea>

      <!-- Live swatch preview for the previewed variant -->
      <div
        class="w-52 shrink-0 overflow-y-auto rounded-md border border-border bg-card/40 px-3 py-2"
      >
        {#each PALETTE_GROUPS as group (group.id)}
          <h3
            class="mb-1 mt-2 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground first:mt-0"
          >
            {group.label}
          </h3>
          {#each group.keys as key (key)}
            <div class="flex items-center gap-2 py-0.5" title={key}>
              <span
                class="h-3.5 w-3.5 shrink-0 rounded-sm border border-border/60"
                style="background: {draft[variant][key]}"
              ></span>
              <span class="flex-1 truncate text-[11px] text-foreground"
                >{key}</span
              >
              <span class="font-mono text-[10px] text-muted-foreground"
                >{draft[variant][key]}</span
              >
            </div>
          {/each}
        {/each}
      </div>
    </div>

    <div class="shrink-0">
      {#if jsonError}
        <p class="text-xs text-error">{jsonError}</p>
      {:else}
        <p class="text-xs text-muted-foreground">
          Valid theme JSON — previewing live. Both light and dark palettes are
          required.
        </p>
      {/if}
    </div>
  </div>

  {#snippet footer()}
    <button
      type="button"
      onclick={onClose}
      class="rounded-md px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
    >
      Cancel
    </button>
    <button
      type="button"
      onclick={handleSave}
      disabled={saveDisabled}
      class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:opacity-90 disabled:opacity-50"
    >
      {isNew ? "Create theme" : "Save changes"}
    </button>
  {/snippet}
</Modal>
