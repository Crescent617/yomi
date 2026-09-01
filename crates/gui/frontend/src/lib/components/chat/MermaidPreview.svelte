<!-- eslint-disable svelte/no-dom-manipulating -->
<script lang="ts">
  import { onMount } from "svelte";
  import { Minus, Plus, RotateCcw, Scan, X } from "lucide-svelte";
  import { isTopModal, pushModal } from "../../modal-stack";

  const modalId = Symbol("mermaid-preview");

  let { svg, onClose }: { svg: string; onClose: () => void } = $props();

  let dialog = $state<HTMLDivElement | null>(null);
  let viewport = $state<HTMLDivElement | null>(null);
  let closeButton = $state<HTMLButtonElement | null>(null);
  let scale = $state(1);
  let panX = $state(0);
  let panY = $state(0);
  let dragging = $state(false);
  let fitted = $state(true);
  let dragStartX = 0;
  let dragStartY = 0;
  let panStartX = 0;
  let panStartY = 0;

  const diagramSize = $derived.by(() => {
    const match = svg.match(/viewBox=["']([^"']+)["']/i);
    const values = match?.[1].trim().split(/[ ,]+/).map(Number);
    if (values?.length === 4 && values.every(Number.isFinite)) {
      return { width: Math.max(values[2], 1), height: Math.max(values[3], 1) };
    }
    return { width: 800, height: 600 };
  });

  function clampScale(value: number) {
    return Math.min(4, Math.max(0.05, value));
  }

  function zoomAt(factor: number, clientX?: number, clientY?: number) {
    const nextScale = clampScale(scale * factor);
    if (nextScale === scale) return;
    if (viewport && clientX !== undefined && clientY !== undefined) {
      const rect = viewport.getBoundingClientRect();
      const x = clientX - (rect.left + rect.width / 2) - panX;
      const y = clientY - (rect.top + rect.height / 2) - panY;
      const ratio = nextScale / scale;
      panX -= x * (ratio - 1);
      panY -= y * (ratio - 1);
    }
    scale = nextScale;
    fitted = false;
  }

  function actualSize() {
    scale = 1;
    panX = 0;
    panY = 0;
    fitted = false;
  }

  function fitToViewport() {
    if (!viewport) return;
    const padding = 64;
    const availableWidth = Math.max(1, viewport.clientWidth - padding);
    const availableHeight = Math.max(1, viewport.clientHeight - padding);
    scale = Math.min(
      availableWidth / diagramSize.width,
      availableHeight / diagramSize.height,
      2,
    );
    panX = 0;
    panY = 0;
    fitted = true;
  }

  function handleWheel(event: WheelEvent) {
    event.preventDefault();
    zoomAt(event.deltaY < 0 ? 1.12 : 1 / 1.12, event.clientX, event.clientY);
  }

  function startDrag(event: PointerEvent) {
    if (event.button !== 0) return;
    dragging = true;
    fitted = false;
    dragStartX = event.clientX;
    dragStartY = event.clientY;
    panStartX = panX;
    panStartY = panY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function moveDrag(event: PointerEvent) {
    if (!dragging) return;
    panX = panStartX + event.clientX - dragStartX;
    panY = panStartY + event.clientY - dragStartY;
  }

  function stopDrag() {
    dragging = false;
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      // Only the top-most overlay layer may consume Escape — a preview
      // stacked above (or below) keeps its own state untouched.
      if (!isTopModal(modalId)) return;
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === "Tab" && dialog) {
      const focusable = [
        ...dialog.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        ),
      ].filter((element) => !element.hasAttribute("disabled"));
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
      return;
    }
    if ((event.metaKey || event.ctrlKey) && event.key === "0") {
      event.preventDefault();
      fitToViewport();
    }
    if (event.key === "+" || event.key === "=") zoomAt(1.15);
    if (event.key === "-") zoomAt(1 / 1.15);
  }

  onMount(() => {
    const previousOverflow = document.body.style.overflow;
    const previousFocus = document.activeElement as HTMLElement | null;
    document.body.style.overflow = "hidden";
    const popModal = pushModal(modalId);
    window.addEventListener("keydown", handleKeydown);
    const resizeObserver = new ResizeObserver(() => {
      if (fitted) fitToViewport();
    });
    if (viewport) resizeObserver.observe(viewport);
    requestAnimationFrame(() => {
      fitToViewport();
      closeButton?.focus();
    });
    return () => {
      document.body.style.overflow = previousOverflow;
      popModal();
      window.removeEventListener("keydown", handleKeydown);
      resizeObserver.disconnect();
      previousFocus?.focus();
    };
  });
</script>

<div
  bind:this={dialog}
  class="fixed inset-0 z-[100] flex flex-col bg-background/95 backdrop-blur-sm"
  role="dialog"
  aria-modal="true"
  aria-label="Mermaid diagram preview"
>
  <div class="flex h-11 shrink-0 items-center border-b border-border px-3">
    <span class="text-xs font-medium text-foreground">Diagram preview</span>
    <div class="ml-auto flex items-center gap-1">
      <button
        type="button"
        onclick={() => zoomAt(1 / 1.15)}
        class="preview-button"
        title="Zoom out"
        aria-label="Zoom out"
      >
        <Minus size={15} />
      </button>
      <button
        type="button"
        onclick={actualSize}
        class="min-w-14 rounded-md px-2 py-1 text-[11px] tabular-nums text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        title="Actual size"
      >
        {Math.round(scale * 100)}%
      </button>
      <button
        type="button"
        onclick={() => zoomAt(1.15)}
        class="preview-button"
        title="Zoom in"
        aria-label="Zoom in"
      >
        <Plus size={15} />
      </button>
      <span class="mx-1 h-4 w-px bg-border"></span>
      <button
        type="button"
        onclick={fitToViewport}
        class="preview-button"
        title="Fit to window (Ctrl/⌘+0)"
        aria-label="Fit to window"
      >
        <Scan size={15} />
      </button>
      <button
        type="button"
        onclick={actualSize}
        class="preview-button"
        title="Reset to 100%"
        aria-label="Reset zoom"
      >
        <RotateCcw size={14} />
      </button>
      <button
        bind:this={closeButton}
        type="button"
        onclick={onClose}
        class="preview-button ml-1"
        title="Close (Esc)"
        aria-label="Close preview"
      >
        <X size={16} />
      </button>
    </div>
  </div>

  <div
    bind:this={viewport}
    role="application"
    aria-label="Interactive Mermaid diagram preview"
    class="relative min-h-0 flex-1 overflow-hidden bg-code-bg/40 {dragging
      ? 'cursor-grabbing'
      : 'cursor-grab'}"
    onwheel={handleWheel}
    onpointerdown={startDrag}
    onpointermove={moveDrag}
    onpointerup={stopDrag}
    onpointercancel={stopDrag}
  >
    <div
      class="preview-diagram absolute left-1/2 top-1/2"
      style={`width: ${diagramSize.width}px; height: ${diagramSize.height}px; transform: translate(-50%, -50%) translate(${panX}px, ${panY}px) scale(${scale});`}
    >
      {@html svg}
    </div>
    <div
      class="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-background/80 px-2.5 py-1 text-[10px] text-muted-foreground shadow-sm backdrop-blur"
    >
      Scroll to zoom · Drag to pan
    </div>
  </div>
</div>

<style>
  .preview-button {
    display: inline-flex;
    width: 1.75rem;
    height: 1.75rem;
    align-items: center;
    justify-content: center;
    border-radius: 0.375rem;
    color: hsl(var(--muted-foreground));
    transition:
      color 150ms,
      background-color 150ms;
  }
  .preview-button:hover {
    color: hsl(var(--foreground));
    background: hsl(var(--secondary));
  }
  .preview-button:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 1px;
  }
  .preview-diagram :global(svg) {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
