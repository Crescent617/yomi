<script lang="ts">
  import { toasts, removeToast } from "../../toast.svelte";
  import { X, Info, CheckCircle, AlertTriangle, AlertCircle } from "lucide-svelte";

  const iconMap = {
    info: Info,
    success: CheckCircle,
    warning: AlertTriangle,
    error: AlertCircle,
  };

  const colorMap = {
    info: "border-blue-500/20 bg-blue-500/10 text-blue-700 dark:text-blue-300",
    success: "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    warning: "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    error: "border-red-500/20 bg-red-500/10 text-red-700 dark:text-red-300",
  };

  const iconColorMap = {
    info: "text-blue-500",
    success: "text-emerald-500",
    warning: "text-amber-500",
    error: "text-red-500",
  };
</script>

<div class="fixed top-4 left-1/2 -translate-x-1/2 z-[100] flex flex-col gap-2 max-w-sm w-full pointer-events-none">
  {#each toasts as toast (toast.id)}
    {@const Icon = iconMap[toast.type]}
    <div
      class="pointer-events-auto flex items-start gap-2.5 rounded-lg border px-3.5 py-2.5 shadow-lg backdrop-blur-sm transition-all duration-300 {colorMap[toast.type]}"
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
