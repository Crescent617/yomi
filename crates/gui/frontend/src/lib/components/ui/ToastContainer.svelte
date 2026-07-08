<script lang="ts">
  import { fly, fade } from "svelte/transition";
  import { flip } from "svelte/animate";
  import { toasts, removeToast } from "../../toast.svelte";
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
  class="fixed top-4 right-4 z-[100] flex flex-col gap-2 w-full max-w-xs pointer-events-none"
>
  {#each toasts as toast (toast.id)}
    {@const Icon = iconMap[toast.type]}
    <div
      in:fly={{ x: 80, duration: 300, opacity: 0 }}
      out:fade={{ duration: 200 }}
      animate:flip={{ duration: 250 }}
      class="pointer-events-auto flex items-start gap-2.5 rounded-lg border px-3.5 py-2.5 shadow-lg {colorMap[
        toast.type
      ]}"
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
</div>
