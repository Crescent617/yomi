<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    getSession,
    requestActivePanel,
    sessionState,
    startNotificationListener,
  } from "../lib/state.svelte";
  import { EventFrameBuffer } from "../lib/event-frame-buffer";
  import ToastContainer from "../lib/components/ui/ToastContainer.svelte";
  import ImagePreview from "../lib/components/ui/ImagePreview.svelte";
  import FilePreviewOverlay from "../lib/components/ui/FilePreviewOverlay.svelte";
  import {
    initSettings,
    guiPreferences,
    startThemeListener,
    stopThemeListener,
  } from "../lib/settings.svelte";
  import * as api from "../lib/api";
  import { refreshConnectionInfo } from "../lib/connection.svelte";
  import "../app.css";

  let appWindow: ReturnType<typeof getCurrentWindow> | undefined;
  let isPetWindow = false;

  // @ts-expect-error svelte onMount 返回类型在 lib 升级后被误判
  onMount(async () => {
    if (typeof window === "undefined") return;

    appWindow = getCurrentWindow();
    isPetWindow = appWindow.label === "pet";
    document.documentElement.classList.toggle("pet-window", isPetWindow);
    document.body.classList.toggle("pet-window", isPetWindow);

    if (isPetWindow) {
      return () => {
        document.documentElement.classList.remove("pet-window");
        document.body.classList.remove("pet-window");
      };
    }

    await initSettings();
    startThemeListener();

    const { activateSession } = await import("../lib/session");
    const { handleEvent } = await import("../lib/events");
    const packs = await api.listPetPacks().catch((error) => {
      console.error("Failed to list desktop pet packs at startup:", error);
      return [];
    });
    const persisted_pet_id = guiPreferences.desktop_pet.selected_pet_id;
    const selected_pet_id = packs.some((pack) => pack.id === persisted_pet_id)
      ? persisted_pet_id
      : (packs[0]?.id ?? null);
    await api.selectPetPack(selected_pet_id).catch((error) => {
      console.error("Failed to restore desktop pet pack:", error);
    });
    await api.setPetScale(guiPreferences.desktop_pet.scale).catch((error) => {
      console.error("Failed to restore desktop pet scale:", error);
    });
    await api
      .setPetEnabled(
        guiPreferences.desktop_pet.enabled && selected_pet_id !== null,
      )
      .catch((error) => {
        console.error("Failed to restore desktop pet preference:", error);
      });
    await api.setKeepAwake(guiPreferences.power.keep_awake).catch((error) => {
      console.error("Failed to restore keep-awake preference:", error);
    });

    const eventFrameBuffer = new EventFrameBuffer(
      ({
        session_id,
        event_id,
        event,
      }: import("../lib/event-frame-buffer").KernelEventEnvelope) => {
        const session = getSession(session_id);
        if (session) {
          handleEvent(session_id, event_id, event);
        }
      },
    );
    const unlistenEvent = listen(
      "kernel:event",
      (e: {
        payload: import("../lib/event-frame-buffer").KernelEventEnvelope;
      }) => {
        eventFrameBuffer.enqueue(e.payload);
      },
    );
    const unlistenNoti = startNotificationListener();
    void refreshConnectionInfo();
    const unlistenOpenSession = listen<{ session_id: string }>(
      "app:open_session",
      (event) => {
        const { session_id } = event.payload;
        if (!requestActivePanel("chat")) return;
        void activateSession(session_id).catch(() => {
          // activateSession reports the error and restores prior state.
        });
      },
    );
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
      unlistenOpenSession.then((fn: () => void) => fn());
      unlistenClose();
      stopThemeListener();
    };
  });
</script>

<div
  class="h-screen w-screen overflow-hidden text-foreground {isPetWindow
    ? 'bg-transparent'
    : 'bg-background'}"
>
  <slot />
  {#if !isPetWindow}<ToastContainer /><ImagePreview /><FilePreviewOverlay
    />{/if}
</div>
