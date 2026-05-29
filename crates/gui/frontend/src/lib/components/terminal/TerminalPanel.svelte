<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { oneDarkTheme, oneLightTheme } from "../../terminal/xtermTheme";

  let {
    id,
    cwd,
    onClose,
  }: {
    id: string;
    cwd: string;
    onClose?: () => void;
  } = $props();

  let container: HTMLElement;
  let term: Terminal;
  let fitAddon: FitAddon;
  let unlisten: (() => void) | null = null;

  onMount(async () => {
    const isDark = document.documentElement.classList.contains("dark");

    term = new Terminal({
      fontFamily: "JetBrains Mono, monospace",
      fontSize: 14,
      theme: isDark ? oneDarkTheme : oneLightTheme,
      cursorBlink: true,
      scrollback: 10000,
    });

    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    fitAddon.fit();

    // Spawn PTY
    await invoke("terminal_spawn", {
      id,
      cwd,
      cols: term.cols,
      rows: term.rows,
    });

    // Forward input to PTY
    term.onData((data) => {
      invoke("terminal_write", { id, data }).catch(console.error);
    });

    // Receive PTY output
    const l = await listen("terminal:data", (e: any) => {
      if (e.payload.id === id) {
        term.write(e.payload.data);
      }
    });
    unlisten = l;

    // Resize handling (debounced)
    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeTimeout) clearTimeout(resizeTimeout);
      resizeTimeout = setTimeout(() => {
        fitAddon.fit();
        invoke("terminal_resize", {
          id,
          cols: term.cols,
          rows: term.rows,
        }).catch(console.error);
      }, 150);
    });
    resizeObserver.observe(container);

    return () => {
      if (resizeTimeout) clearTimeout(resizeTimeout);
      resizeObserver.disconnect();
    };
  });

  onDestroy(() => {
    unlisten?.();
    term?.dispose();
    invoke("terminal_kill", { id }).catch(console.error);
  });
</script>

<div bind:this={container} class="h-full w-full bg-terminal"></div>
