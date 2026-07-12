<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    getSession,
    sessionState,
    startNotificationListener,
  } from "../lib/state.svelte";
  import { handleEvent } from "../lib/events";
  import {
    EventFrameBuffer,
    type KernelEventEnvelope,
  } from "../lib/event-frame-buffer";
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
    const eventFrameBuffer = new EventFrameBuffer(
      ({ session_id, event_id, event }: KernelEventEnvelope) => {
        const session = getSession(session_id);
        if (session) {
          handleEvent(session_id, event_id, event);
        }
      },
    );
    const unlistenEvent = listen(
      "kernel:event",
      (e: { payload: KernelEventEnvelope }) => {
        eventFrameBuffer.enqueue(e.payload);
      },
    );
    const unlistenNoti = startNotificationListener();
    const appWindow = getCurrentWindow();
    const unlistenClose = await appWindow.onCloseRequested(() => {
      try {
        eventFrameBuffer.flush();
        const active = sessionState.activeSessionId;
        if (active) {
          api.unsubscribe(active);
        }
      } catch (e) {
        console.error("Error in onCloseRequested:", e);
      }
    });
    return () => {
      eventFrameBuffer.dispose();
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
