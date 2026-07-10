<script lang="ts">
  import {
    X,
    Info,
    CheckCircle,
    AlertTriangle,
    AlertCircle,
  } from "lucide-svelte";
  import type { Toast } from "../../toast.svelte";

  let {
    toast,
    onDismiss,
  }: {
    toast: Toast;
    onDismiss: () => void;
  } = $props();

  const iconMap = {
    info: Info,
    success: CheckCircle,
    warning: AlertTriangle,
    error: AlertCircle,
  };

  const colorMap = {
    info: "border-info/20 bg-[color-mix(in_oklab,var(--color-info)_10%,var(--color-background))] text-info",
    success:
      "border-success/20 bg-[color-mix(in_oklab,var(--color-success)_10%,var(--color-background))] text-success",
    warning:
      "border-warning/20 bg-[color-mix(in_oklab,var(--color-warning)_10%,var(--color-background))] text-warning",
    error:
      "border-error/20 bg-[color-mix(in_oklab,var(--color-error)_10%,var(--color-background))] text-error",
  };

  const iconColorMap = {
    info: "text-info",
    success: "text-success",
    warning: "text-warning",
    error: "text-error",
  };

  const Icon = $derived(iconMap[toast.type]);
</script>

<div
  class="pointer-events-auto relative z-[3] flex w-full items-start gap-2.5 rounded-lg border px-3.5 py-2.5 shadow-lg {colorMap[
    toast.type
  ]}"
  role={toast.type === "error" ? "alert" : "status"}
>
  <Icon size={18} class="mt-0.5 shrink-0 {iconColorMap[toast.type]}" />
  <span class="flex-1 text-sm leading-snug">{toast.message}</span>
  <button
    type="button"
    onclick={onDismiss}
    aria-label="Dismiss notification"
    class="-mr-1 -mt-0.5 shrink-0 rounded-md p-1 opacity-60 transition-all hover:bg-secondary hover:opacity-100"
  >
    <X size={14} />
  </button>
</div>
