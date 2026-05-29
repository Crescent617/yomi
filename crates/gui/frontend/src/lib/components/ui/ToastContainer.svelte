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
    info: "bg-blue-500/10 text-blue-600 border-blue-500/20",
    success: "bg-emerald-500/10 text-emerald-600 border-emerald-500/20",
    warning: "bg-amber-500/10 text-amber-600 border-amber-500/20",
    error: "bg-red-500/10 text-red-600 border-red-500/20",
  };
</script>

<div class="fixed bottom-4 right-4 z-[100] flex flex-col gap-2 max-w-sm w-full">
  {#each toasts as toast (toast.id)}
    <div
      class="flex items-start gap-2 rounded-lg border px-3 py-2 shadow-lg backdrop-blur transition-all {colorMap[toast.type]}"
    >
      <svelte:component this={iconMap[toast.type]} size={16} class="shrink-0 mt-0.5" />
      <span class="flex-1 text-sm">{toast.message}</span>
      <button
        onclick={() => removeToast(toast.id)}
        class="shrink-0 opacity-70 hover:opacity-100 transition-opacity"
      >
        <X size={14} />
      </button>
    </div>
  {/each}
</div>
