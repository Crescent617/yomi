<script lang="ts">
  import {
    Check,
    ChevronDown,
    ChevronUp,
    Loader2,
    ShieldAlert,
    ShieldX,
  } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const permissions = $derived(activeSession?.pending_permissions ?? []);
  const permission = $derived(permissions[0]);

  let pendingAction = $state<"deny" | "once" | "always" | null>(null);
  let showArguments = $state(false);
  let activeRequestId = $state<string | null>(null);

  $effect(() => {
    if (permission?.req_id !== activeRequestId) {
      activeRequestId = permission?.req_id ?? null;
      pendingAction = null;
      showArguments = false;
    }
  });

  const formattedArguments = $derived(
    permission?.tool_args ? formatJson(permission.tool_args) : "",
  );
  const argumentSummary = $derived(
    permission?.tool_args ? summarizeArguments(permission.tool_args) : "",
  );
  const toolLabel = $derived(
    permission?.tool_name ? capitalize(permission.tool_name) : "Tool",
  );

  function capitalize(value: string): string {
    return value ? value.charAt(0).toUpperCase() + value.slice(1) : value;
  }

  function formatJson(value: string): string {
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  }

  function summarizeArguments(value: string): string {
    try {
      const parsed = JSON.parse(value) as Record<string, unknown>;
      const preferredKeys = [
        "command",
        "path",
        "file_path",
        "url",
        "query",
        "pattern",
      ];
      for (const key of preferredKeys) {
        const item = parsed[key];
        if (typeof item === "string" && item.trim()) return item.trim();
      }
      const first = Object.values(parsed).find(
        (item) => typeof item === "string" && item.trim(),
      );
      if (typeof first === "string") return first.trim();
    } catch {
      return value.trim();
    }
    return "";
  }

  async function respond(allow: boolean, remember: boolean) {
    if (!activeSession || !permission || pendingAction) return;
    const action = allow ? (remember ? "always" : "once") : "deny";
    pendingAction = action;
    const requestId = permission.req_id;
    const sessionId = permission.session_id || activeSession.id;

    try {
      await api.respondPermission(sessionId, requestId, allow, remember);
      // Keep the request visible until PermissionAck confirms the agent received it.
    } catch (e: unknown) {
      showNotification(
        `${allow ? "Approval" : "Denial"} failed: ${api.errorMessage(e)}`,
        "error",
      );
      pendingAction = null;
    }
  }
</script>

{#if permission}
  <div
    class="shrink-0 border-t border-border bg-background px-3 py-2.5 sm:px-4"
  >
    <section
      class="relative overflow-hidden rounded-lg border border-warning/25 bg-warning/5 shadow-sm"
      aria-labelledby="permission-title"
    >
      <span
        class="absolute inset-y-0 left-0 w-0.5 bg-warning"
        aria-hidden="true"
      ></span>

      <div class="px-3.5 py-3 pl-4">
        <div class="flex items-start justify-between gap-3">
          <div class="flex min-w-0 items-start gap-2.5">
            <div
              class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-warning/10 text-warning"
            >
              <ShieldAlert class="size-4" />
            </div>
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <h2 id="permission-title" class="text-sm font-medium">
                  Approval required
                </h2>
                <span
                  class="rounded-full bg-warning/10 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-warning"
                >
                  {permission.tool_level}
                </span>
                {#if permissions.length > 1}
                  <span class="text-[11px] tabular-nums text-muted-foreground">
                    1 of {permissions.length}
                  </span>
                {/if}
              </div>
              <p class="mt-0.5 text-xs text-muted-foreground">
                <code class="font-mono text-foreground">{toolLabel}</code>
                {permission.reason || " wants permission to continue."}
              </p>
            </div>
          </div>
        </div>

        {#if argumentSummary}
          <button
            type="button"
            onclick={() => (showArguments = !showArguments)}
            class="mt-3 flex w-full items-center gap-2 rounded-md bg-code-bg px-3 py-2 text-left font-mono text-xs text-foreground transition-colors hover:bg-secondary/70 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            aria-expanded={showArguments}
          >
            <span class="min-w-0 flex-1 truncate">{argumentSummary}</span>
            {#if showArguments}
              <ChevronUp class="size-3.5 shrink-0 text-muted-foreground" />
            {:else}
              <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
            {/if}
          </button>
          {#if showArguments}
            <pre
              class="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-code-bg px-3 py-2 font-mono text-[11px] leading-relaxed text-muted-foreground">{formattedArguments}</pre>
          {/if}
        {/if}

        <div class="mt-3 flex flex-wrap items-center justify-end gap-1.5">
          <button
            type="button"
            onclick={() => respond(false, false)}
            disabled={pendingAction !== null}
            class="inline-flex h-8 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-xs font-medium text-muted-foreground transition-colors hover:border-destructive/30 hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          >
            {#if pendingAction === "deny"}
              <Loader2 class="size-3.5 animate-spin" />
            {:else}
              <ShieldX class="size-3.5" />
            {/if}
            Deny
          </button>
          <button
            type="button"
            onclick={() => respond(true, true)}
            disabled={pendingAction !== null}
            class="inline-flex h-8 items-center rounded-md border border-warning/25 bg-background px-3 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
            title="Allow this tool automatically for the rest of the session"
          >
            {#if pendingAction === "always"}
              <Loader2 class="mr-1.5 size-3.5 animate-spin" />
            {/if}
            Always allow
          </button>
          <button
            type="button"
            onclick={() => respond(true, false)}
            disabled={pendingAction !== null}
            class="inline-flex h-8 items-center gap-1.5 rounded-md border border-warning/30 bg-warning/10 px-3 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          >
            {#if pendingAction === "once"}
              <Loader2 class="size-3.5 animate-spin" />
            {:else}
              <Check class="size-3.5" />
            {/if}
            Allow once
          </button>
        </div>
      </div>
    </section>
  </div>
{/if}
