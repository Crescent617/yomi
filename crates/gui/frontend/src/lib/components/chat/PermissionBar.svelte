<script lang="ts">
  import { ShieldCheck, ShieldX } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const permissions = $derived(activeSession?.pending_permissions ?? []);

  async function approve(req_id: string, remember: boolean) {
    if (!activeSession) return;
    try {
      await api.respondPermission(activeSession.id, req_id, true, remember);
      activeSession.pending_permissions =
        activeSession.pending_permissions.filter((p) => p.req_id !== req_id);
    } catch (e: unknown) {
      showNotification(
        "Approval failed: " + (e instanceof Error ? e.message : ""),
        "error",
        3000,
      );
    }
  }

  async function deny(req_id: string) {
    if (!activeSession) return;
    try {
      await api.respondPermission(activeSession.id, req_id, false, false);
      activeSession.pending_permissions =
        activeSession.pending_permissions.filter((p) => p.req_id !== req_id);
    } catch (e: unknown) {
      showNotification(
        "Denial failed: " + (e instanceof Error ? e.message : ""),
        "error",
        3000,
      );
    }
  }

  function compactJson(s: string): string {
    try {
      const obj = JSON.parse(s);
      return JSON.stringify(obj, null, 2);
    } catch {
      return s;
    }
  }
</script>

{#if permissions.length > 0}
  <div
    class="shrink-0 border-t border-border bg-amber-50/40 dark:bg-amber-950/20 px-4 py-3 space-y-3"
  >
    {#each permissions as perm (perm.req_id)}
      <div
        class="rounded-lg border border-amber-200 dark:border-amber-800 bg-background px-3 py-2.5"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="flex-1 min-w-0">
            <div
              class="flex items-center gap-2 text-sm font-medium text-amber-700 dark:text-amber-400"
            >
              <ShieldCheck class="w-4 h-4 shrink-0" />
              <span class="truncate">{perm.tool_name}</span>
              <span
                class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-amber-100 dark:bg-amber-900 text-amber-700 dark:text-amber-400"
              >
                {perm.tool_level}
              </span>
            </div>
            {#if perm.reason}
              <p class="text-xs text-muted-foreground mt-1">{perm.reason}</p>
            {/if}
            {#if perm.tool_args}
              <pre
                class="mt-1.5 text-[10px] bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap max-h-48 overflow-y-auto">{compactJson(
                  perm.tool_args,
                )}</pre>
            {/if}
          </div>
          <div class="flex items-center gap-1.5 shrink-0">
            <button
              type="button"
              onclick={() => deny(perm.req_id)}
              class="px-2.5 py-1 rounded-md border border-border text-xs font-medium text-muted-foreground hover:bg-destructive/10 hover:text-destructive hover:border-destructive/30 transition-colors"
            >
              <ShieldX class="w-3 h-3 inline mr-1" />
              Deny
            </button>
            <button
              type="button"
              onclick={() => approve(perm.req_id, false)}
              class="px-2.5 py-1 rounded-md bg-amber-600 text-white text-xs font-medium hover:bg-amber-700 active:scale-95 transition-colors"
            >
              <ShieldCheck class="w-3 h-3 inline mr-1" />
              Approve
            </button>
            <button
              type="button"
              onclick={() => approve(perm.req_id, true)}
              class="px-2 py-1 rounded-md border border-amber-200 dark:border-amber-700 text-[10px] text-amber-700 dark:text-amber-400 hover:bg-amber-100 dark:hover:bg-amber-900 transition-colors"
              title="Always allow this tool"
            >
              Always
            </button>
          </div>
        </div>
      </div>
    {/each}
  </div>
{/if}
