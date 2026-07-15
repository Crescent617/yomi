<script lang="ts">
  import { Bell, CheckCheck, Inbox } from "lucide-svelte";
  import {
    markAllSessionNotificationsRead,
    markSessionNotificationRead,
    projectState,
    requestActivePanel,
    sessionNotifications,
    sessionState,
  } from "../../state.svelte";
  import { activateSession } from "../../session";
  import { clock } from "../../clock.svelte";
  import { relativeNotificationTime } from "../../notification-center";

  let open = $state(false);
  let buttonRef = $state<HTMLButtonElement>();
  let panelRef = $state<HTMLDivElement>();

  const unreadCount = $derived(
    sessionNotifications.filter((notification) => !notification.read).length,
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

  async function openNotification(id: string, sessionId: string) {
    if (!requestActivePanel("chat")) return;
    try {
      await activateSession(sessionId);
      if (sessionState.activeSessionId !== sessionId) return;
      markSessionNotificationRead(id);
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
    class="relative grid size-5 place-items-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    aria-expanded={open}
    aria-controls="notification-center-panel"
    aria-haspopup="dialog"
    aria-label={unreadCount > 0
      ? `Notifications, ${unreadCount} unread`
      : "Notifications"}
    title="Notifications"
    onclick={() => {
      if (open) closePanel();
      else open = true;
    }}
  >
    <Bell class="size-3.5" aria-hidden="true" />
    {#if unreadCount > 0}
      <span
        class="absolute -right-1.5 -top-1.5 min-w-3.5 rounded-full bg-primary px-1 text-center text-[9px] font-semibold leading-3.5 text-primary-foreground shadow-sm"
      >
        {unreadCount > 99 ? "99+" : unreadCount}
      </span>
    {/if}
  </button>

  {#if open}
    <div
      id="notification-center-panel"
      role="dialog"
      aria-label="Notifications"
      bind:this={panelRef}
      class="absolute bottom-full right-0 z-40 mb-2 w-[22rem] max-w-[calc(100vw-1.5rem)] overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-2xl"
    >
      <header
        class="flex items-center justify-between border-b border-border px-3 py-2"
      >
        <div class="flex min-w-0 items-baseline gap-2">
          <h2 class="text-sm font-semibold">Notifications</h2>
          {#if unreadCount > 0}
            <span class="text-[11px] text-muted-foreground">
              {unreadCount} unread
            </span>
          {/if}
        </div>
        {#if unreadCount > 0}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            onclick={markAllSessionNotificationsRead}
          >
            <CheckCheck class="size-3" />
            Mark all read
          </button>
        {/if}
      </header>

      {#if sessionNotifications.length === 0}
        <div class="grid place-items-center px-6 py-10 text-center">
          <div
            class="mb-3 grid size-9 place-items-center rounded-full bg-secondary text-muted-foreground"
          >
            <Inbox class="size-4" />
          </div>
          <p class="text-sm font-medium">No notifications yet</p>
          <p class="mt-1 max-w-52 text-xs leading-5 text-muted-foreground">
            Completed background sessions will appear here.
          </p>
        </div>
      {:else}
        <div class="max-h-80 overflow-y-auto py-1">
          {#each sessionNotifications as notification (notification.id)}
            {@const project = projectName(notification.projectId)}
            <button
              type="button"
              class="group flex w-full items-start gap-2 px-3 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
              onclick={() =>
                void openNotification(notification.id, notification.sessionId)}
            >
              <span
                class="mt-1.5 size-1.5 shrink-0 rounded-full {notification.read
                  ? 'bg-transparent'
                  : 'bg-primary'}"
                aria-label={notification.read ? undefined : "Unread"}
              ></span>
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2">
                  <span
                    class="min-w-0 flex-1 truncate text-xs {notification.read
                      ? 'font-normal text-muted-foreground'
                      : 'font-medium text-foreground'}"
                  >
                    {notification.title}
                  </span>
                  <time
                    datetime={notification.completedAt}
                    class="shrink-0 text-[10px] tabular-nums text-muted-foreground/80"
                    title={new Date(notification.completedAt).toLocaleString()}
                  >
                    {relativeNotificationTime(
                      notification.completedAt,
                      clock.now,
                    )}
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
        </div>
      {/if}
    </div>
  {/if}
</div>
