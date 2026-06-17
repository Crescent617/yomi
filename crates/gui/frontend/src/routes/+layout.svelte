<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    handleEvent,
    getSession,
    unsubscribeAllInactive,
  } from "../lib/state.svelte";
  import ToastContainer from "../lib/components/ui/ToastContainer.svelte";
  import {
    initSettings,
    startThemeListener,
    stopThemeListener,
  } from "../lib/settings.svelte";
  import "../app.css";

  // @ts-expect-error svelte onMount 返回类型在 lib 升级后被误判
  onMount(async () => {
    await initSettings();
    startThemeListener();
    const unlisten = listen(
      "kernel:event",
      (e: { payload: { session_id: string; event: unknown } }) => {
        const { session_id, event } = e.payload;
        const session = getSession(session_id);
        if (session) {
          handleEvent(session_id, event);
        }
      },
    );
    const appWindow = getCurrentWindow();
    const unlistenClose = await appWindow.onCloseRequested(() => {
      try {
        unsubscribeAllInactive();
      } catch (e) {
        console.error("Error in onCloseRequested:", e);
      }
    });
    return () => {
      unlisten.then((fn: () => void) => fn());
      unlistenClose();
      stopThemeListener();
    };
  });
</script>

<div class="h-screen w-screen bg-background text-foreground overflow-hidden">
  <slot />
  <ToastContainer />
</div>
