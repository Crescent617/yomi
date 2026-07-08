<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    handleEvent,
    getSession,
    sessionState,
    startNotificationListener,
  } from "../lib/state.svelte";
  import ToastContainer from "../lib/components/ui/ToastContainer.svelte";
  import {
    initSettings,
    startThemeListener,
    stopThemeListener,
  } from "../lib/settings.svelte";
  import * as api from "../lib/api";
  import "../app.css";

  // @ts-expect-error svelte onMount 返回类型在 lib 升级后被误判
  onMount(async () => {
    await initSettings();
    startThemeListener();
    const unlistenEvent = listen(
      "kernel:event",
      (e: {
        payload: { session_id: string; event_id?: string; event: unknown };
      }) => {
        const { session_id, event_id, event } = e.payload;
        const session = getSession(session_id);
        if (session) {
          handleEvent(session_id, event_id, event);
        }
      },
    );
    const unlistenNoti = startNotificationListener();
    const appWindow = getCurrentWindow();
    const unlistenClose = await appWindow.onCloseRequested(() => {
      try {
        const active = sessionState.activeSessionId;
        if (active) {
          api.unsubscribe(active);
        }
      } catch (e) {
        console.error("Error in onCloseRequested:", e);
      }
    });
    return () => {
      unlistenEvent.then((fn: () => void) => fn());
      unlistenNoti.then((fn: () => void) => fn());
      unlistenClose();
      stopThemeListener();
    };
  });
</script>

<div class="h-screen w-screen bg-background text-foreground overflow-hidden">
  <slot />
  <ToastContainer />
</div>
