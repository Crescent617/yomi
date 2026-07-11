<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import type { SessionInfo } from "../../api";
  import {
    projectState,
    sessionState,
    activateSession,
    refreshCheckpoints,
    createSessionState,
    showNotification,
  } from "../../state.svelte";
  import { formatTimeAgo, projectColor } from "../../utils";
  import { clock } from "../../clock.svelte";
  import { MessageSquare, History } from "lucide-svelte";

  let recent = $state<SessionInfo[]>([]);
  let loaded = $state(false);
  let opening = $state<string | null>(null);

  onMount(() => {
    // Fetch a few extra so we still have 3 after filtering out
    // sessions without a project (e.g. subagents, ad-hoc dirs).
    api
      .listSessions(undefined, undefined, 10)
      .then((r) => {
        recent = r.sessions.filter((s) => !!s.project_id).slice(0, 3);
        loaded = true;
      })
      .catch(() => {
        loaded = true;
      });
  });

  function projectOf(s: SessionInfo) {
    if (s.project_id) {
      return projectState.projects.find((p) => p.id === s.project_id) ?? null;
    }
    return null;
  }

  async function resume(s: SessionInfo) {
    if (opening) return;
    opening = s.id;
    try {
      if (!sessionState.sessions.find((sess) => sess.id === s.id)) {
        sessionState.sessions.push(
          createSessionState({
            id: s.id,
            project_path: s.project_path ?? "",
            project_id: s.project_id,
            alias: s.title ?? "Untitled",
            updated_at: s.updated_at ?? s.created_at,
            permission_level: s.auto_approve_level ?? "caution",
            model_key: s.model_key,
          }),
        );
      }
      await activateSession(s.id);
      refreshCheckpoints(s.id);
    } catch (e: unknown) {
      console.error(
        "Failed to resume session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to open session", "error");
    } finally {
      opening = null;
    }
  }
</script>

{#if loaded && recent.length > 0}
  <div class="space-y-1.5">
    <div
      class="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground uppercase tracking-wider px-1"
    >
      <History class="w-3 h-3" />
      Recent sessions
    </div>
    <div class="grid gap-1.5 sm:grid-cols-3">
      {#each recent as s (s.id)}
        {@const project = projectOf(s)}
        <button
          type="button"
          onclick={() => resume(s)}
          disabled={opening !== null}
          class="group flex flex-col gap-1 rounded-md border border-border/80 bg-card/40 px-3 py-2 text-left
                 hover:border-primary/40 hover:bg-card transition-all disabled:opacity-60 min-w-0"
        >
          <span class="flex items-center gap-1.5 min-w-0">
            {#if opening === s.id}
              <span
                class="w-3 h-3 border-2 border-primary/30 border-t-primary rounded-full animate-spin shrink-0"
              ></span>
            {:else}
              <MessageSquare
                class="w-3 h-3 text-muted-foreground group-hover:text-primary transition-colors shrink-0"
              />
            {/if}
            <span class="text-xs font-medium truncate">
              {s.title || "Untitled"}
            </span>
          </span>
          <span
            class="flex items-center gap-1.5 text-[10px] text-muted-foreground min-w-0"
          >
            {#if project}
              <span
                class="w-1.5 h-1.5 rounded-full shrink-0"
                style="background: {projectColor(project.name + project.dir)}"
              ></span>
              <span class="truncate">{project.name}</span>
              <span>·</span>
            {/if}
            <span class="shrink-0"
              >{formatTimeAgo(s.updated_at ?? s.created_at, clock.now)}</span
            >
          </span>
        </button>
      {/each}
    </div>
  </div>
{/if}
