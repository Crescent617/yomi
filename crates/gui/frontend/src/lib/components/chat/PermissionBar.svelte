<script lang="ts">
  import { ShieldCheck, ShieldX } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const permissions = $derived(activeSession?.pendingPermissions ?? []);

  async function approve(reqId: string, remember: boolean) {
    if (!activeSession) return;
    try {
      await api.respondPermission(activeSession.id, reqId, true, remember);
      activeSession.pendingPermissions = activeSession.pendingPermissions.filter((p) => p.reqId !== reqId);
    } catch (e: unknown) {
      showNotification("Approval failed: " + (e instanceof Error ? e.message : ""), "error", 3000);
    }
  }

  async function deny(reqId: string) {
    if (!activeSession) return;
    try {
      await api.respondPermission(activeSession.id, reqId, false, false);
      activeSession.pendingPermissions = activeSession.pendingPermissions.filter((p) => p.reqId !== reqId);
    } catch (e: unknown) {
      showNotification("Denial failed: " + (e instanceof Error ? e.message : ""), "error", 3000);
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
  <div class="shrink-0 border-t border-border bg-amber-50/40 dark:bg-amber-950/20 px-4 py-3 space-y-3">
    {#each permissions as perm (perm.reqId)}
      <div class="rounded-lg border border-amber-200 dark:border-amber-800 bg-background px-3 py-2.5">
        <div class="flex items-start justify-between gap-3">
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2 text-sm font-medium text-amber-700 dark:text-amber-400">
              <ShieldCheck class="w-4 h-4 shrink-0" />
              <span class="truncate">{perm.toolName}</span>
              <span class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-amber-100 dark:bg-amber-900 text-amber-700 dark:text-amber-400">
                {perm.toolLevel}
              </span>
            </div>
            {#if perm.reason}
              <p class="text-xs text-muted-foreground mt-1">{perm.reason}</p>
            {/if}
            {#if perm.toolArgs}
              <pre class="mt-1.5 text-[10px] bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap">{compactJson(perm.toolArgs)}</pre>
            {/if}
          </div>
          <div class="flex items-center gap-1.5 shrink-0">
            <button
              type="button"
              onclick={() => deny(perm.reqId)}
              class="px-2.5 py-1 rounded-md border border-border text-xs font-medium text-muted-foreground hover:bg-destructive/10 hover:text-destructive hover:border-destructive/30 transition-colors"
            >
              <ShieldX class="w-3 h-3 inline mr-1" />
              Deny
            </button>
            <button
              type="button"
              onclick={() => approve(perm.reqId, false)}
              class="px-2.5 py-1 rounded-md bg-amber-600 text-white text-xs font-medium hover:bg-amber-700 active:scale-95 transition-colors"
            >
              <ShieldCheck class="w-3 h-3 inline mr-1" />
              Approve
            </button>
            <button
              type="button"
              onclick={() => approve(perm.reqId, true)}
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
