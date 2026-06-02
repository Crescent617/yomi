<script lang="ts">
  // @ts-ignore
  import { sessionState, getActiveSession } from "$lib/state.svelte";

  const activeSession = $derived(getActiveSession());
</script>

<p>Direct: {sessionState.activeSessionId}</p>
<p>Getter: {activeSession}</p>
<button onclick={() => sessionState.activeSessionId = sessionState.activeSessionId ? null : 'test'}>
  Toggle
</button>
