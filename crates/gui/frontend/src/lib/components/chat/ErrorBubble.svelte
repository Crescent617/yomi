<script lang="ts">
  import { AlertCircle } from "lucide-svelte";
  import type { ChatMessage } from "../../state.svelte";

  let { message }: { message: ChatMessage } = $props();
</script>

<div class="flex justify-center my-2">
  <div class="max-w-[85%] flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
    <AlertCircle size={14} class="mt-0.5 shrink-0" />
    <span class="leading-relaxed">{message.content}</span>
  </div>
</div>
