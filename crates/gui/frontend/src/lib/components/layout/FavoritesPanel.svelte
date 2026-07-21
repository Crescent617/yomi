<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    Star,
    Search,
    RefreshCw,
    Copy,
    Check,
    Share2,
    ExternalLink,
    Trash2,
    ArrowLeft,
  } from "lucide-svelte";
  import * as api from "../../api";
  import type { FavoriteAnswer } from "../../api";
  import {
    favoritesState,
    loadFavorites,
    deleteFavorite,
    saveFavoriteNote,
  } from "../../favorites.svelte";
  import { activateSession } from "../../session";
  import {
    requestActivePanel,
    requestScrollToMessage,
    showNotification,
  } from "../../state.svelte";
  import { markdownToPlainText } from "../../share-text";
  import { requestShare } from "../../share.svelte";
  import { formatMessageTime } from "../../utils";
  import TextBlock from "../chat/TextBlock.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  let selectedId = $state<string | null>(null);
  let query = $state("");
  let searchResults = $state<FavoriteAnswer[] | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let refreshing = $state(false);
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  let deleteTarget = $state<FavoriteAnswer | null>(null);
  let deleting = $state(false);

  const visibleItems = $derived(searchResults ?? favoritesState.items);
  const selected = $derived(
    visibleItems.find((item) => item.id === selectedId) ?? null,
  );
  // Writable derived: typing overrides locally, switching favorites resets.
  let noteDraft = $derived(selected?.note ?? "");

  onMount(() => {
    void loadFavorites(true);
  });

  onDestroy(() => {
    clearTimeout(copyTimer);
    clearTimeout(searchTimer);
  });

  // Keep the selection valid as the visible list changes. Auto-select the
  // first entry only on desktop where list and detail are visible together;
  // on mobile auto-selecting would fight the back-to-list navigation.
  $effect(() => {
    const items = visibleItems;
    if (items.length === 0) {
      selectedId = null;
      return;
    }
    if (
      window.innerWidth >= 1024 &&
      !items.some((item) => item.id === selectedId)
    ) {
      selectedId = items[0].id;
    }
  });

  function plainText(item: FavoriteAnswer): string {
    return markdownToPlainText(item.content);
  }

  function firstLine(item: FavoriteAnswer): string {
    return (
      plainText(item)
        .split("\n")
        .find((l) => l.trim().length > 0) ?? ""
    );
  }

  function onQueryInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      const q = query.trim();
      if (!q) {
        searchResults = null;
        return;
      }
      try {
        searchResults = await api.listFavorites(q, 200, 0);
      } catch (e) {
        console.error("Failed to search favorites:", e);
        showNotification("Failed to search favorites", "error");
      }
    }, 300);
  }

  async function refresh() {
    refreshing = true;
    try {
      await loadFavorites(true);
      if (query.trim()) {
        searchResults = await api.listFavorites(query.trim(), 200, 0);
      }
    } finally {
      refreshing = false;
    }
  }

  async function copySelected() {
    if (!selected) return;
    try {
      await navigator.clipboard.writeText(selected.content);
      clearTimeout(copyTimer);
      copied = true;
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch (e) {
      console.error("Failed to copy:", e);
      showNotification("Failed to copy", "error");
    }
  }

  function shareSelected() {
    if (!selected) return;
    requestShare({
      content: selected.content,
      sessionTitle: selected.session_title,
      date: selected.message_created_at
        ? new Date(selected.message_created_at)
        : new Date(selected.favorited_at),
    });
  }

  async function jumpToSource() {
    if (!selected) return;
    const target = selected;
    try {
      requestActivePanel("chat");
      // activateSession already surfaces failures via an error toast.
      await activateSession(target.session_id);
      requestScrollToMessage(target.message_id);
    } catch (e) {
      console.error("Failed to open source session:", e);
    }
  }

  function saveNote() {
    if (!selected) return;
    const note = noteDraft.trim();
    if (note === (selected.note ?? "")) return;
    void saveFavoriteNote(selected.id, note || undefined);
    searchResults =
      searchResults?.map((item) =>
        item.id === selected.id ? { ...item, note: note || undefined } : item,
      ) ?? null;
  }

  async function confirmDelete() {
    if (!deleteTarget || deleting) return;
    deleting = true;
    const target = deleteTarget;
    try {
      await deleteFavorite(target.id);
      searchResults =
        searchResults?.filter((item) => item.id !== target.id) ?? null;
      deleteTarget = null;
    } finally {
      deleting = false;
    }
  }

  const iconBtnClass =
    "inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50";
</script>

<div class="flex h-full w-full flex-col">
  <header
    class="flex h-14 shrink-0 items-center justify-between gap-3 border-b border-border px-4 lg:px-6"
  >
    <div class="flex min-w-0 items-center gap-2">
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={onToggleLeftPanel}
          class="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary/80 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring lg:hidden"
          title="Toggle sidebar"
          aria-label="Toggle sidebar"
        >
          <svg
            class="size-5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="M3 12h18M3 6h18M3 18h18" /></svg
          >
        </button>
      {/if}
      <Star class="size-5 shrink-0 text-warning" fill="currentColor" />
      <h1 class="truncate text-lg font-semibold">Favorites</h1>
      <span class="hidden text-xs text-muted-foreground sm:inline">
        {visibleItems.length} saved
      </span>
    </div>

    <div class="flex shrink-0 items-center gap-1.5">
      <div class="relative">
        <Search
          class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <input
          type="text"
          bind:value={query}
          oninput={onQueryInput}
          placeholder="Search favorites…"
          aria-label="Search favorites"
          class="h-8 w-40 rounded-md border border-input bg-background pl-8 pr-3 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring sm:w-56"
        />
      </div>
      <button
        type="button"
        onclick={refresh}
        disabled={refreshing}
        class={iconBtnClass}
        title="Refresh favorites"
        aria-label="Refresh favorites"
      >
        <RefreshCw class="size-4 {refreshing ? 'animate-spin' : ''}" />
      </button>
    </div>
  </header>

  <div class="flex min-h-0 flex-1 overflow-hidden">
    <aside
      class="{selected
        ? 'hidden lg:flex'
        : 'flex'} w-full shrink-0 flex-col overflow-hidden border-r border-border lg:w-80 xl:w-96"
      aria-label="Favorite answers"
    >
      <div
        class="flex h-10 shrink-0 items-center justify-between border-b border-border px-4 text-xs text-muted-foreground"
      >
        <span class="font-medium uppercase tracking-wide">Answers</span>
        <span>{visibleItems.length}</span>
      </div>

      {#if visibleItems.length === 0}
        <div
          class="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center"
        >
          <div
            class="flex size-11 items-center justify-center rounded-full bg-secondary text-muted-foreground"
          >
            <Star class="size-5" />
          </div>
          <div>
            <p class="text-sm font-medium">
              {query.trim() ? "No matches" : "No favorites yet"}
            </p>
            <p class="mt-1 max-w-56 text-xs text-muted-foreground">
              {query.trim()
                ? "Try a different search term."
                : "Hover an assistant answer and click the star to save it here."}
            </p>
          </div>
        </div>
      {:else}
        <div class="flex-1 overflow-y-auto">
          {#each visibleItems as item (item.id)}
            <button
              type="button"
              onclick={() => (selectedId = item.id)}
              class="group relative w-full border-b border-border/50 px-4 py-3 text-left transition-colors hover:bg-secondary/40 focus-visible:z-10 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring {selectedId ===
              item.id
                ? 'bg-primary/5'
                : ''}"
            >
              {#if selectedId === item.id}
                <span
                  class="absolute inset-y-2 left-0 w-0.5 rounded-r bg-primary"
                  aria-hidden="true"
                ></span>
              {/if}
              <div class="flex items-center gap-2">
                <Star
                  class="size-3.5 shrink-0 text-warning"
                  fill="currentColor"
                />
                <span class="min-w-0 flex-1 truncate text-sm font-medium">
                  {item.note || firstLine(item)}
                </span>
              </div>
              {#if item.note}
                <p class="mt-1 line-clamp-2 text-xs text-muted-foreground">
                  {firstLine(item)}
                </p>
              {/if}
              <div
                class="mt-1.5 flex items-center gap-2 text-[11px] text-muted-foreground"
              >
                <span class="truncate">
                  {item.session_title ?? "Deleted session"}
                </span>
                <span aria-hidden="true">·</span>
                <time class="shrink-0" datetime={item.favorited_at}>
                  {formatMessageTime(item.favorited_at)}
                </time>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </aside>

    <section
      class="{selected ? 'flex' : 'hidden lg:flex'} min-w-0 flex-1 flex-col"
      aria-label="Favorite detail"
    >
      {#if selected}
        <header
          class="flex h-14 shrink-0 items-center justify-between gap-3 border-b border-border px-4 lg:px-6"
        >
          <div class="flex min-w-0 items-center gap-2">
            <button
              type="button"
              onclick={() => (selectedId = null)}
              class="{iconBtnClass} lg:hidden"
              title="Back to list"
              aria-label="Back to list"
            >
              <ArrowLeft class="size-4" />
            </button>
            <div class="min-w-0">
              <h2 class="truncate text-base font-semibold">
                {selected.note || firstLine(selected)}
              </h2>
              <div
                class="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <span class="truncate">
                  {selected.session_title ?? "Deleted session"}
                </span>
                <span aria-hidden="true">·</span>
                <time class="shrink-0" datetime={selected.favorited_at}>
                  {formatMessageTime(selected.favorited_at)}
                </time>
              </div>
            </div>
          </div>

          <div class="flex shrink-0 items-center gap-1">
            <button
              type="button"
              onclick={copySelected}
              class={iconBtnClass}
              title={copied ? "Copied" : "Copy as markdown"}
              aria-label={copied ? "Copied" : "Copy as markdown"}
            >
              {#if copied}
                <Check class="size-4 text-success" />
              {:else}
                <Copy class="size-4" />
              {/if}
            </button>
            <button
              type="button"
              onclick={shareSelected}
              class={iconBtnClass}
              title="Share as image"
              aria-label="Share as image"
            >
              <Share2 class="size-4" />
            </button>
            <button
              type="button"
              onclick={jumpToSource}
              class={iconBtnClass}
              title="Open source session"
              aria-label="Open source session"
            >
              <ExternalLink class="size-4" />
            </button>
            <button
              type="button"
              onclick={() => (deleteTarget = selected)}
              class="{iconBtnClass} hover:bg-destructive/10 hover:text-destructive"
              title="Remove favorite"
              aria-label="Remove favorite"
            >
              <Trash2 class="size-4" />
            </button>
          </div>
        </header>

        <div class="shrink-0 border-b border-border/50 px-4 py-2 lg:px-6">
          <input
            type="text"
            bind:value={noteDraft}
            onblur={saveNote}
            onkeydown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            }}
            placeholder="Add a note…"
            aria-label="Favorite note"
            class="h-7 w-full bg-transparent text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none"
          />
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-4 lg:px-6">
          <TextBlock content={selected.content} isStreaming={false} />
        </div>
      {:else}
        <div
          class="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center"
        >
          <div
            class="flex size-11 items-center justify-center rounded-full bg-secondary text-muted-foreground"
          >
            <Star class="size-5" />
          </div>
          <p class="text-sm text-muted-foreground">
            Select a favorite to read it here.
          </p>
        </div>
      {/if}
    </section>
  </div>
</div>

<ConfirmDialog
  open={deleteTarget !== null}
  title="Remove favorite"
  message="Remove this answer from favorites? The original message is not affected."
  confirmText="Remove"
  onConfirm={confirmDelete}
  onCancel={() => (deleteTarget = null)}
/>
