<script lang="ts">
  import { fly, fade } from "svelte/transition";
  import { toasts, removeToast, clearToasts } from "../../toast.svelte";
  import {
    X,
    Info,
    CheckCircle,
    AlertTriangle,
    AlertCircle,
  } from "lucide-svelte";

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
</script>

<div
  class="group fixed top-8 right-4 z-[100] flex flex-col-reverse w-full max-w-xs"
  role="region"
  aria-label="Notifications"
>
  {#each toasts as toast, index (toast.id)}
    {@const Icon = iconMap[toast.type]}
    {@const ri = toasts.length - 1 - index}
    <div
      in:fly={{ x: 80, duration: 300, opacity: 0 }}
      out:fade={{ duration: 200 }}
      class="toast pointer-events-auto w-full rounded-lg border px-3.5 py-2.5 shadow-lg flex items-start gap-2.5 group-hover:mb-2 last:group-hover:mb-0 {colorMap[
        toast.type
      ]}"
      style="--i: {index}; --ri: {ri}"
    >
      <Icon size={18} class="shrink-0 mt-0.5 {iconColorMap[toast.type]}" />
      <span class="flex-1 text-sm leading-snug">{toast.message}</span>
      <button
        onclick={() => removeToast(toast.id)}
        class="shrink-0 -mr-1 -mt-0.5 p-1 rounded-md opacity-60 hover:opacity-100 hover:bg-secondary transition-all"
      >
        <X size={14} />
      </button>
    </div>
  {/each}

  {#if toasts.length > 0}
    <button
      onclick={clearToasts}
      class="pointer-events-none opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 transition-opacity duration-150 absolute -top-5 right-0 z-[110] rounded-md bg-popover border border-subtle px-2 py-0.5 text-xs font-medium text-muted-foreground shadow-sm hover:bg-muted transition-colors"
    >
      Clear all
    </button>
  {/if}
</div>

<style>
  .toast {
    z-index: calc(100 + var(--i));
    transform: translateY(calc(min(var(--ri), 3) * -20px))
      scale(calc(1 - min(var(--ri), 3) * 0.04));
    transition:
      transform 0.3s cubic-bezier(0.4, 0, 0.2, 1),
      margin-bottom 0.3s cubic-bezier(0.4, 0, 0.2, 1);
  }

  .group:hover .toast {
    transform: translateY(0) scale(1);
  }
</style>
