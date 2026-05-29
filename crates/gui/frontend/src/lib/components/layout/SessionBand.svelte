<script lang="ts">
  import { Plus } from "lucide-svelte";
  import * as api from "../../api";
  import {
    sessionState,
    setActiveSession,
    loadSessionMessages,
    getSession,
  } from "../../state.svelte";

  let { collapsed = false }: { collapsed?: boolean } = $props();

  const sessionList = $derived(sessionState.sessions);

  function formatShortId(id: string) {
    return id.slice(0, 8);
  }

  async function activateSession(id: string) {
    const prevId = sessionState.activeSessionId;
    try {
      // Unsubscribe previous session if any
      if (prevId && prevId !== id) {
        await api.unsubscribe(prevId);
      }
      // Subscribe new session (server auto-restores if shutdown)
      await api.subscribe(id);
      setActiveSession(id);
      const raw = await api.getMessages(id);
      const session = getSession(id);
      if (session) {
        loadSessionMessages(id, raw);
      }
    } catch (e: any) {
      console.error("Failed to activate session:", e?.message ?? e);
      // Revert to previous session if activation failed
      if (prevId && prevId !== id) {
        try {
          await api.subscribe(prevId);
          setActiveSession(prevId);
        } catch {
          setActiveSession(null);
        }
      } else {
        setActiveSession(null);
      }
    }
  }
</script>

<div class="flex flex-col gap-1 p-2 overflow-y-auto">
  {#each sessionList as session (session.id)}
    <button
      class="group flex items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors border-l-4 {session.id ===
      sessionState.activeSessionId
        ? 'bg-primary/10 border-primary'
        : 'hover:bg-secondary border-transparent'}"
      onclick={() => activateSession(session.id)}
    >
      {#if !collapsed}
        <span class="flex-1 truncate text-sm font-medium"
          >{session.alias ?? formatShortId(session.id)}</span
        >
        {#if session.unread > 0}
          <span
            class="shrink-0 inline-flex items-center justify-center min-w-[1.25rem] h-5 px-1 rounded-full bg-destructive text-destructive-foreground text-xs font-bold"
          >
            {session.unread}
          </span>
        {/if}
      {/if}
    </button>
  {/each}

  <button
    class="flex items-center gap-2 rounded-lg px-3 py-2 text-sm text-muted-foreground hover:bg-secondary transition-colors"
    onclick={async () => {
      try {
        const cwd = await api.getCwd();
        const newId = await api.createSession(cwd, "safe");
        await activateSession(newId);
      } catch (e: any) {
        console.error("Failed to create session:", e?.message ?? e);
      }
    }}
  >
    <Plus size={16} />
    {#if !collapsed}New Session{/if}
  </button>
</div>
