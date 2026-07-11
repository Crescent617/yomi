<script lang="ts">
  import {
    CheckCircle2,
    AlertCircle,
    Info,
    AlertTriangle,
    X,
  } from "lucide-svelte";
  import type { Toast } from "../../toast.svelte";

  let {
    toast,
    onDismiss,
    compact = false,
  }: {
    toast: Toast;
    onDismiss: () => void;
    compact?: boolean;
  } = $props();

  const icons = {
    success: CheckCircle2,
    error: AlertCircle,
    info: Info,
    warning: AlertTriangle,
  };

  const iconClasses = {
    success: "text-success",
    error: "text-error",
    info: "text-info",
    warning: "text-warning",
  };

  const Icon = $derived(icons[toast.type]);
</script>

<div
  class="pointer-events-auto flex w-full items-start gap-2.5 rounded-lg bg-popover px-3 py-2.5 text-popover-foreground shadow-lg ring-1 ring-border/70"
  class:py-2={compact}
  role={toast.type === "error" ? "alert" : "status"}
>
  <Icon size={16} class="mt-0.5 shrink-0 {iconClasses[toast.type]}" />
  <p class="min-w-0 flex-1 text-xs leading-relaxed">
    {toast.message}
  </p>
  {#if toast.count > 1}
    <span
      class="mt-0.5 shrink-0 rounded bg-secondary px-1.5 py-0.5 font-mono text-[10px] leading-none text-muted-foreground"
      aria-label={`Repeated ${toast.count} times`}
    >
      ×{toast.count}
    </span>
  {/if}
  <button
    type="button"
    class="-mr-1 inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    onclick={(event) => {
      event.stopPropagation();
      onDismiss();
    }}
    aria-label="Dismiss notification"
  >
    <X size={13} />
  </button>
</div>
