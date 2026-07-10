<script lang="ts">
  import { ShieldCheck, ShieldX } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const permissions = $derived(activeSession?.pending_permissions ?? []);

  async function approve(req_id: string, remember: boolean) {
    if (!activeSession) return;
    const perm = activeSession.pending_permissions.find(
      (p) => p.req_id === req_id,
    );
    const sessionId = perm?.session_id || activeSession.id;
    try {
      await api.respondPermission(sessionId, req_id, true, remember);
      activeSession.pending_permissions =
        activeSession.pending_permissions.filter((p) => p.req_id !== req_id);
    } catch (e: unknown) {
      showNotification("Approval failed: " + api.errorMessage(e), "error");
    }
  }

  async function deny(req_id: string) {
    if (!activeSession) return;
    const perm = activeSession.pending_permissions.find(
      (p) => p.req_id === req_id,
    );
    const sessionId = perm?.session_id || activeSession.id;
    try {
      await api.respondPermission(sessionId, req_id, false, false);
      activeSession.pending_permissions =
        activeSession.pending_permissions.filter((p) => p.req_id !== req_id);
    } catch (e: unknown) {
      showNotification("Denial failed: " + api.errorMessage(e), "error");
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
    class="shrink-0 border-t border-border bg-warning/10 px-4 py-3 space-y-3"
  >
    {#each permissions as perm (perm.req_id)}
      <div
        class="rounded-lg border border-warning/20 bg-background px-3 py-2.5"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="flex-1 min-w-0">
            <div
              class="flex items-center gap-2 text-sm font-medium text-warning"
            >
              <ShieldCheck class="w-4 h-4 shrink-0" />
              <span class="truncate">{perm.tool_name}</span>
              <span
                class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-warning/10 text-warning"
              >
                {perm.tool_level}
              </span>
            </div>
            {#if perm.reason}
              <p class="text-xs text-muted-foreground mt-1">{perm.reason}</p>
            {/if}
            {#if perm.tool_args}
              <pre
                class="mt-1.5 text-[10px] bg-code-bg rounded px-2 py-1 whitespace-pre-wrap max-h-48 overflow-y-auto">{compactJson(
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
              class="px-2.5 py-1 rounded-md bg-warning text-warning-foreground text-xs font-medium hover:bg-warning/90 active:scale-95 transition-colors"
            >
              <ShieldCheck class="w-3 h-3 inline mr-1" />
              Approve
            </button>
            <button
              type="button"
              onclick={() => approve(perm.req_id, true)}
              class="px-2 py-1 rounded-md border border-warning/20 text-[10px] text-warning hover:bg-warning/10 transition-colors"
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
