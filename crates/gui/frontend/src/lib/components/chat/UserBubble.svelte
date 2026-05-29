<script lang="ts">
  import type { ChatMessage } from "../../state.svelte";

  let { message }: { message: ChatMessage } = $props();
</script>

<div class="flex justify-end">
  <div class="max-w-[80%] lg:max-w-[70%] rounded-2xl rounded-br-sm bg-secondary px-4 py-3 text-sm">
    {message.content}
  </div>
</div>
