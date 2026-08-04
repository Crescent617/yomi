<script lang="ts">
  import { CheckCheck, Inbox } from "lucide-svelte";
  import { onMount } from "svelte";
  import {
    attentionItems,
    markAllAttentionItemsRead,
    markAttentionItemRead,
    projectState,
    requestActivePanel,
    sessionState,
    streamingSessions,
  } from "../../state.svelte";
  import { activateSession } from "../../session";
  import * as api from "../../api";
  import { clock } from "../../clock.svelte";
  import { relativeTime } from "../../attention-box";
  import { aggregateMood, moodTextClass } from "./status-activity";
  import CodexPetSprite from "../CodexPetSprite.svelte";
  import PopoverPanel from "../ui/PopoverPanel.svelte";
  import { moodToCodexPetAnimation } from "../../codex-pet";

  let open = $state(false);
  let buttonRef = $state<HTMLButtonElement>();
  let panelRef = $state<HTMLDivElement>();

  // ── Pet mood ─────────────────────────────────────────────────────────
  // The pet window computes the full PetMood in Rust; here we approximate
  // the same priority ladder from main-window state (permission > ask >
  // working > idle) so the app feels alive even with the pet disabled.
  let petSpriteUrl = $state<string | null>(null);
  let petSpriteVersion = $state<1 | 2>(1);
  const pendingPermission = $derived(
    sessionState.sessions.some((s) => s.pending_permissions.length > 0),
  );
  const pendingAsk = $derived(
    sessionState.sessions.some((s) => s.pending_ask_users.length > 0),
  );
  const streamingCount = $derived(streamingSessions.length);
  const petMood = $derived(
    aggregateMood({
      pendingPermission,
      pendingAsk,
      runningCount: streamingCount,
    }),
  );

  onMount(() => {
    let cancelledRef = { cancelled: false };
    const loadPetSprite = async () => {
      try {
        // The layout restores the Rust-side pack selection after children
        // mount, so fall back to the first pack when nothing is selected
        // yet — the spritesheet only needs a pack id.
        const pack =
          (await api.getSelectedPetPack()) ??
          (await api.listPetPacks())[0] ??
          null;
        if (!pack || cancelledRef.cancelled) return false;
        const bytes = await api.readSelectedPetSpritesheet(
          pack.id,
          pack.sprite_version_number,
        );
        if (cancelledRef.cancelled) return true;
        petSpriteUrl = URL.createObjectURL(new Blob([bytes]));
        petSpriteVersion = pack.sprite_version_number;
        return true;
      } catch {
        return false;
      }
    };
    void (async () => {
      // One retry covers the boot race where the pack selection is not
      // restored yet; afterwards the chip degrades to text-only.
      if (!(await loadPetSprite()) && !cancelledRef.cancelled) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        if (!cancelledRef.cancelled) await loadPetSprite();
      }
    })();
    return () => {
      cancelledRef.cancelled = true;
      if (petSpriteUrl) URL.revokeObjectURL(petSpriteUrl);
    };
  });

  // ── Attention inbox (completed background sessions) ──────────────────
  const unreadCount = $derived(
    attentionItems.filter((item) => !item.read).length,
  );

  function projectName(projectId: string | null): string | null {
    if (!projectId) return null;
    return (
      projectState.projects.find((project) => project.id === projectId)?.name ??
      null
    );
  }

  function closePanel({ restoreFocus = false } = {}) {
    if (!open) return;
    open = false;
    if (restoreFocus) requestAnimationFrame(() => buttonRef?.focus());
  }

  function closeOnOutsideClick(event: MouseEvent) {
    const target = event.target as Node;
    if (open && !buttonRef?.contains(target) && !panelRef?.contains(target)) {
      closePanel();
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") closePanel({ restoreFocus: true });
  }

  async function openItem(id: string, sessionId: string) {
    if (!requestActivePanel("chat")) return;
    try {
      await activateSession(sessionId);
      if (sessionState.activeSessionId !== sessionId) return;
      markAttentionItemRead(id);
      open = false;
    } catch {
      // activateSession reports the error and restores the previous session.
    }
  }
</script>

<svelte:window onclick={closeOnOutsideClick} onkeydown={handleKeydown} />

<div class="relative flex items-center">
  <button
    bind:this={buttonRef}
    type="button"
    class="relative flex items-center gap-0.5 rounded px-1 py-0.5 transition-colors hover:bg-secondary/70 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    aria-expanded={open}
    aria-controls="attention-box-panel"
    aria-haspopup="dialog"
    aria-label={unreadCount > 0
      ? `Attention box, ${unreadCount} unread, pet mood: ${petMood}`
      : `Attention box, pet mood: ${petMood}`}
    title="Pet mood: {petMood}{streamingCount > 0
      ? ` · ${streamingCount} running`
      : ''}{unreadCount > 0 ? ` · ${unreadCount} unread` : ''}"
    onclick={() => {
      if (open) closePanel();
      else open = true;
    }}
  >
    {#if petSpriteUrl}
      <CodexPetSprite
        src={petSpriteUrl}
        animation={moodToCodexPetAnimation(petMood)}
        scale={0.1}
        sprite_version_number={petSpriteVersion}
        label="Pet mood indicator"
      />
    {/if}
    <span class="micro-label {moodTextClass(petMood)}">{petMood}</span>
    {#if unreadCount > 0}
      <span
        class="absolute -right-1.5 -top-1.5 min-w-3.5 rounded-full bg-primary px-1 text-center text-[9px] font-semibold leading-3.5 text-primary-foreground shadow-sm"
      >
        {unreadCount > 99 ? "99+" : unreadCount}
      </span>
    {/if}
  </button>

  {#if open}
    <PopoverPanel
      bind:ref={panelRef}
      id="attention-box-panel"
      role="dialog"
      aria-label="Attention box"
      title="Attention Box"
      class="absolute bottom-full right-0 z-40 mb-1 w-[22rem] max-w-[calc(100vw-1.5rem)]"
    >
      {#snippet headerActions()}
        {#if unreadCount > 0}
          <span class="text-[10px] text-muted-foreground">
            {unreadCount} unread
          </span>
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            onclick={markAllAttentionItemsRead}
          >
            <CheckCheck class="size-3" />
            Mark all read
          </button>
        {/if}
      {/snippet}

      {#if attentionItems.length === 0}
        <div class="grid place-items-center px-6 py-10 text-center">
          <div
            class="mb-3 grid size-9 place-items-center rounded-full bg-secondary text-muted-foreground"
          >
            <Inbox class="size-4" />
          </div>
          <p class="text-sm font-medium">Nothing needs your attention</p>
          <p class="mt-1 max-w-52 text-xs leading-5 text-muted-foreground">
            Completed background sessions will appear here.
          </p>
        </div>
      {:else}
        {#each attentionItems as item (item.id)}
          {@const project = projectName(item.projectId)}
          <button
            type="button"
            class="popover-list-item flex w-full items-start gap-2 px-3 py-2 text-left"
            onclick={() => void openItem(item.id, item.sessionId)}
          >
            <span
              class="mt-1.5 size-1.5 shrink-0 rounded-full {item.read
                ? 'bg-transparent'
                : 'bg-primary'}"
              aria-label={item.read ? undefined : "Unread"}
            ></span>
            <span class="min-w-0 flex-1">
              <span class="flex items-center gap-2">
                <span
                  class="min-w-0 flex-1 truncate text-xs {item.read
                    ? 'font-normal text-muted-foreground'
                    : 'font-medium text-foreground'}"
                >
                  {item.title}
                </span>
                <time
                  datetime={item.completedAt}
                  class="shrink-0 text-[10px] tabular-nums text-muted-foreground/80"
                  title={new Date(item.completedAt).toLocaleString()}
                >
                  {relativeTime(item.completedAt, clock.now)}
                </time>
              </span>
              {#if project}
                <span
                  class="mt-0.5 block truncate text-[10px] text-muted-foreground"
                >
                  {project}
                </span>
              {/if}
            </span>
          </button>
        {/each}
      {/if}
    </PopoverPanel>
  {/if}
</div>
