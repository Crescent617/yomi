<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { handleEvent, getSession } from "../lib/state.svelte";
  import ToastContainer from "../lib/components/ui/ToastContainer.svelte";
  import { applyTheme, settings, startThemeListener } from "../lib/settings.svelte";
  import "../app.css";

  // Client-side only — avoid SSR crash
  if (typeof document !== "undefined") {
    applyTheme(settings.theme);
  }

  onMount(() => {
    applyTheme(settings.theme);
    startThemeListener();
    const unlisten = listen("kernel:event", (e: any) => {
      const { sessionId, event } = e.payload;
      const session = getSession(sessionId);
      if (session) {
        handleEvent(sessionId, event);
      }
    });
    return () => {
      unlisten.then((fn: any) => fn());
    };
  });
</script>

<div class="h-screen w-screen bg-background text-foreground overflow-hidden">
  <slot />
  <ToastContainer />
</div>
