<script lang="ts">
  import type { UserQueryMarker } from "./query-navigator";

  let {
    scrollContainer,
    messageContent,
    queries,
    onJump,
  }: {
    scrollContainer: HTMLDivElement | null;
    messageContent: HTMLDivElement | null;
    queries: UserQueryMarker[];
    onJump: () => void;
  } = $props();

  let activeId = $state<string | null>(null);
  let expanded = $state(false);
  let anchors = new Map<string, HTMLElement>();
  let measureFrame: number | null = null;

  function collectAnchors() {
    measureFrame = null;
    if (!messageContent) {
      anchors = new Map();
      activeId = null;
      return;
    }

    const next = new Map<string, HTMLElement>();
    for (const anchor of messageContent.querySelectorAll<HTMLElement>(
      "[data-user-query-id]",
    )) {
      const id = anchor.dataset.userQueryId;
      if (id && !next.has(id)) next.set(id, anchor);
    }
    anchors = next;
    updateActiveQuery();
  }

  function scheduleCollectAnchors() {
    if (measureFrame !== null) return;
    measureFrame = requestAnimationFrame(collectAnchors);
  }

  function updateActiveQuery() {
    if (!scrollContainer || anchors.size === 0) {
      activeId = null;
      return;
    }

    const containerTop = scrollContainer.getBoundingClientRect().top;
    const readingLine = containerTop + scrollContainer.clientHeight * 0.2;
    const distanceFromBottom = Math.max(
      0,
      scrollContainer.scrollHeight -
        scrollContainer.scrollTop -
        scrollContainer.clientHeight,
    );
    let active = queries.find((query) => anchors.has(query.id));
    if (distanceFromBottom <= 1) {
      active = [...queries].reverse().find((query) => anchors.has(query.id));
    } else {
      for (const query of queries) {
        const anchor = anchors.get(query.id);
        if (!anchor || anchor.getBoundingClientRect().top > readingLine) break;
        active = query;
      }
    }
    activeId = active?.id ?? null;
  }

  function jumpTo(query: UserQueryMarker) {
    const anchor = anchors.get(query.id);
    if (!scrollContainer || !anchor) return;

    const containerTop = scrollContainer.getBoundingClientRect().top;
    const offset =
      anchor.getBoundingClientRect().top -
      containerTop +
      scrollContainer.scrollTop;
    const reduceMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    onJump();
    scrollContainer.scrollTo({
      top: Math.max(0, offset - 16),
      behavior: reduceMotion ? "auto" : "smooth",
    });
    activeId = query.id;
  }

  function closeWhenFocusLeaves(event: FocusEvent) {
    const next = event.relatedTarget;
    if (
      !(next instanceof Node) ||
      !(event.currentTarget as HTMLElement).contains(next)
    ) {
      expanded = false;
    }
  }

  $effect(() => {
    const container = scrollContainer;
    const content = messageContent;
    queries.map((query) => query.id).join("|");
    if (!container || !content) return;

    const resizeObserver = new ResizeObserver(scheduleCollectAnchors);
    resizeObserver.observe(content);
    container.addEventListener("scroll", updateActiveQuery, { passive: true });
    scheduleCollectAnchors();

    return () => {
      resizeObserver.disconnect();
      container.removeEventListener("scroll", updateActiveQuery);
      if (measureFrame !== null) cancelAnimationFrame(measureFrame);
      measureFrame = null;
    };
  });
</script>

{#if queries.length >= 2}
  <nav
    class="group/query-nav absolute right-2 top-1/2 z-20 hidden -translate-y-1/2 md:block"
    class:pointer-events-none={expanded}
    aria-label="User query navigator"
    data-expanded={expanded}
    onmouseenter={() => (expanded = true)}
    onmouseleave={() => (expanded = false)}
    onfocusin={() => (expanded = true)}
    onfocusout={closeWhenFocusLeaves}
  >
    <div
      class="pointer-events-auto absolute right-0 top-1/2 flex w-80 max-h-[min(28rem,calc(100vh-6rem))] -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border/40 bg-background p-1.5 shadow-sm transition-transform duration-200 ease-out will-change-transform motion-reduce:transition-none {expanded
        ? 'translate-x-0'
        : 'translate-x-[calc(100%+0.5rem)]'}"
      class:pointer-events-auto={expanded}
      class:pointer-events-none={!expanded}
      aria-hidden={!expanded}
    >
      <ol class="query-navigator-list min-h-0 space-y-0.5 overflow-y-auto">
        {#each queries as query (query.id)}
          <li>
            <button
              type="button"
              class="group/query relative flex w-full items-start overflow-hidden rounded-lg py-2 pl-5 pr-2.5 text-left text-xs leading-relaxed transition-[background-color,color] duration-200 ease-out hover:bg-secondary/70 focus-visible:bg-secondary/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none {query.id ===
              activeId
                ? 'bg-secondary/45'
                : ''}"
              aria-current={query.id === activeId ? "location" : undefined}
              tabindex={expanded ? 0 : -1}
              onclick={() => jumpTo(query)}
            >
              <span
                class="absolute left-2 top-1/2 h-0.5 -translate-y-1/2 rounded-full transition-[width,background-color,opacity] duration-200 ease-out group-hover/query:w-2.5 group-hover/query:bg-primary group-hover/query:opacity-100 group-focus-visible/query:w-2.5 group-focus-visible/query:bg-primary group-focus-visible/query:opacity-100 motion-reduce:transition-none {query.id ===
                activeId
                  ? 'w-2 bg-primary opacity-100'
                  : 'w-1 bg-muted-foreground/40 opacity-60'}"
                aria-hidden="true"
              ></span>
              <span
                class="line-clamp-2 min-w-0 text-foreground/85 transition-colors duration-200 group-hover/query:text-foreground group-focus-visible/query:text-foreground motion-reduce:transition-none"
                >{query.label}</span
              >
            </button>
          </li>
        {/each}
      </ol>
    </div>

    <div
      class="pointer-events-auto flex max-h-[min(28rem,calc(100vh-6rem))] min-h-9 min-w-6 cursor-default flex-col items-end justify-center gap-0.5 overflow-y-auto rounded-md px-1.5 py-1 transition-colors hover:bg-secondary/70 motion-reduce:transition-none"
      aria-label={`Show all ${queries.length} user queries`}
    >
      {#each queries as query (query.id)}
        <button
          type="button"
          class="flex h-1.5 min-w-3 shrink-0 items-center justify-end rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={`Jump to query: ${query.label}`}
          title={`Jump to query: ${query.label}`}
          tabindex={expanded ? -1 : 0}
          onclick={(event) => {
            event.stopPropagation();
            jumpTo(query);
          }}
        >
          <span
            class="h-0.5 rounded-full transition-[width,background-color] motion-reduce:transition-none {query.id ===
            activeId
              ? 'w-3 bg-primary'
              : 'w-2 bg-muted-foreground/45'}"
            aria-hidden="true"
          ></span>
        </button>
      {/each}
    </div>
  </nav>
{/if}

<style>
  .query-navigator-list {
    scrollbar-width: none;
  }

  .query-navigator-list::-webkit-scrollbar {
    display: none;
  }
</style>
