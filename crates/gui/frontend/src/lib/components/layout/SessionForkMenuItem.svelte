<script lang="ts">
  import { GitFork } from "lucide-svelte";

  let {
    session_id,
    disabled = false,
    onfork,
  }: {
    session_id: string;
    disabled?: boolean;
    onfork: (session_id: string) => void;
  } = $props();
</script>

<button
  type="button"
  class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-foreground transition-colors hover:bg-primary hover:text-primary-foreground focus-visible:bg-primary focus-visible:text-primary-foreground focus-visible:outline-none disabled:opacity-50"
  {disabled}
  onclick={(event) => {
    event.stopPropagation();
    onfork(session_id);
  }}
>
  <GitFork size={12} /> Fork
</button>
