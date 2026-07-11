<script lang="ts">
  import type { Message, SessionState } from "../../state.svelte";
  import { findThinking, hasText } from "../../state.svelte";

  let {
    session,
    messages,
  }: {
    session: SessionState;
    messages: Message[];
  } = $props();

  const currentToolName = $derived.by(() => {
    if (session.streaming_tool_name) return session.streaming_tool_name;
    if (session.phase !== "streaming" && session.phase !== "executing_tool") {
      return null;
    }

    for (let i = messages.length - 1; i >= 0; i--) {
      const message = messages[i];
      if (message.type === "tool") {
        if (message.status === "running") return message.tool_name;
        continue;
      }
      if (message.type === "assistant") {
        if (findThinking(message.content) || hasText(message.content))
          return null;
        return message.tool_calls?.at(-1)?.name ?? null;
      }
      return null;
    }
    return null;
  });

  const capitalize = (value: string) =>
    value ? value.charAt(0).toUpperCase() + value.slice(1) : value;

  const status = $derived.by(() => {
    if (session.phase === "compacting") return "Compacting";
    if (currentToolName) return "Calling";

    for (let i = messages.length - 1; i >= 0; i--) {
      const message = messages[i];
      if (message.type === "assistant") {
        const lastBlock = message.content.at(-1);
        if (lastBlock?.type === "text" && lastBlock.text) return "Writing";
        return "Thinking";
      }
      if (message.type === "tool") return "Thinking";
      if (message.type === "user" || message.type === "error") {
        return "Thinking";
      }
    }
    return "Thinking";
  });

  const displayToolName = $derived(
    currentToolName ? capitalize(currentToolName) : null,
  );

  const visible = $derived(
    session.is_running ||
      session.phase === "streaming" ||
      session.phase === "executing_tool" ||
      session.phase === "compacting",
  );
</script>

{#if visible}
  <div
    class="inline-flex min-h-5 -mt-1.5 items-center gap-2 pl-0.5 text-[12px] font-normal text-muted-foreground/75"
    role="status"
    aria-live="polite"
    aria-atomic="true"
  >
    <span
      class="stream-track relative h-2 w-[18px] shrink-0 overflow-hidden"
      aria-hidden="true"
    >
      <span
        class="absolute left-0 right-0 top-1/2 h-px -translate-y-1/2 bg-primary/15"
      ></span>
      <span class="stream-comet absolute inset-y-0 left-0 w-3">
        <span
          class="absolute left-0 top-1/2 h-px w-2.5 -translate-y-1/2 bg-gradient-to-r from-transparent via-primary/45 to-primary/80"
        ></span>
        <span
          class="absolute right-0 top-1/2 size-1.5 -translate-y-1/2 rounded-full bg-primary"
        ></span>
      </span>
    </span>
    {#key `${status}-${displayToolName ?? ""}`}
      <span class="status-label">
        {status}{#if displayToolName}
          <span class="font-mono font-medium text-primary/85"
            >&nbsp;{displayToolName}</span
          >
        {/if}...
      </span>
    {/key}
  </div>
{/if}

<style>
  @keyframes stream-flow {
    0% {
      opacity: 0;
      transform: translateX(-0.75rem);
    }
    18% {
      opacity: 1;
    }
    82% {
      opacity: 1;
    }
    100% {
      opacity: 0;
      transform: translateX(1.125rem);
    }
  }

  @keyframes status-enter {
    from {
      opacity: 0;
      transform: translateX(-0.125rem);
    }
    to {
      opacity: 1;
      transform: translateX(0);
    }
  }

  .stream-comet {
    animation: stream-flow 1.15s ease-in-out infinite;
    will-change: transform, opacity;
  }

  .status-label {
    animation: status-enter 0.16s ease-out;
  }

  @media (prefers-reduced-motion: reduce) {
    .stream-comet,
    .status-label {
      animation: none;
    }

    .stream-comet {
      opacity: 1;
      transform: translateX(0.375rem);
    }
  }
</style>
