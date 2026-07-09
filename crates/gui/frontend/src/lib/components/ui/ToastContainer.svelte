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
    info: "border-blue-500/20 bg-blue-50 dark:bg-blue-950 text-blue-700 dark:text-blue-300",
    success:
      "border-emerald-500/20 bg-emerald-50 dark:bg-emerald-950 text-emerald-700 dark:text-emerald-300",
    warning:
      "border-amber-500/20 bg-amber-50 dark:bg-amber-950 text-amber-700 dark:text-amber-300",
    error:
      "border-red-500/20 bg-red-50 dark:bg-red-950 text-red-700 dark:text-red-300",
  };

  const iconColorMap = {
    info: "text-blue-500",
    success: "text-emerald-500",
    warning: "text-amber-500",
    error: "text-red-500",
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
      class="toast pointer-events-auto w-full rounded-lg border px-3.5 py-2.5 shadow-lg flex items-start gap-2.5 opacity-[calc(1-var(--ri)*0.35)] group-hover:opacity-100 group-hover:mb-2 last:group-hover:mb-0 {colorMap[
        toast.type
      ]}"
      style="--i: {index}; --ri: {ri}"
    >
      <Icon size={18} class="shrink-0 mt-0.5 {iconColorMap[toast.type]}" />
      <span class="flex-1 text-sm leading-snug">{toast.message}</span>
      <button
        onclick={() => removeToast(toast.id)}
        class="shrink-0 -mr-1 -mt-0.5 p-1 rounded-md opacity-60 hover:opacity-100 hover:bg-black/5 dark:hover:bg-white/10 transition-all"
      >
        <X size={14} />
      </button>
    </div>
  {/each}

  {#if toasts.length > 0}
    <button
      onclick={clearToasts}
      class="pointer-events-none opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 transition-opacity duration-150 absolute -top-5 right-0 z-[110] rounded-md bg-white/80 dark:bg-black/60 backdrop-blur-sm border border-black/10 dark:border-white/10 px-2 py-0.5 text-xs font-medium text-slate-600 dark:text-slate-300 shadow-sm hover:bg-white dark:hover:bg-black/80 transition-colors"
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
  }

  .group:hover .toast {
    transition:
      transform 0.3s cubic-bezier(0.4, 0, 0.2, 1),
      opacity 0.3s cubic-bezier(0.4, 0, 0.2, 1),
      margin-bottom 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    transform: translateY(0) scale(1);
  }
</style>
