<script lang="ts">
  import type { Message, SessionState } from "../../state.svelte";
  import { findThinking, hasText } from "../../session";
  import { isActiveSessionPhase, noteRunStart } from "../../session-phase";
  import { humanizeToolName } from "../tool/tool-utils";
  import {
    estimateJsonTokens,
    estimateTextTokens,
    extractPartialTarget,
    formatRunElapsed,
    formatStreamTokens,
    toolVerb,
  } from "./stream-status";

  let {
    session,
    messages,
  }: {
    session: SessionState;
    messages: Message[];
  } = $props();

  /** The tool currently running (or streaming its arguments), with raw args. */
  const currentTool = $derived.by((): { name: string; args: string } | null => {
    if (session.streaming_tool_name) {
      const lastMsg = messages.at(-1);
      const args =
        lastMsg?.type === "assistant"
          ? (lastMsg.tool_calls?.at(-1)?.arguments ?? "")
          : "";
      return { name: session.streaming_tool_name, args };
    }
    if (session.phase !== "streaming" && session.phase !== "executing_tool") {
      return null;
    }

    for (let i = messages.length - 1; i >= 0; i--) {
      const message = messages[i];
      if (message.type === "tool") {
        if (message.status === "running") {
          return { name: message.tool_name, args: message.arguments };
        }
        continue;
      }
      if (message.type === "assistant") {
        if (findThinking(message.content) || hasText(message.content))
          return null;
        const call = message.tool_calls?.at(-1);
        return call ? { name: call.name, args: call.arguments } : null;
      }
      return null;
    }
    return null;
  });

  const status = $derived.by(() => {
    if (session.phase === "compacting") return "Compacting";
    if (currentTool) return "Calling";

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

  /** Present-tense verb for the current tool, null when not in a tool. */
  const verb = $derived(currentTool ? toolVerb(currentTool.name) : null);
  /** Leading status word: tool verb, or Thinking/Writing/Compacting. */
  const leadWord = $derived(
    currentTool && verb && verb !== "Calling" ? verb : status,
  );
  /** Accent beside the lead word: tool target, or the humanized name of
   *  tools without a known verb. */
  const target = $derived(
    currentTool ? extractPartialTarget(currentTool.name, currentTool.args) : "",
  );
  const accent = $derived(
    currentTool && verb && verb !== "Calling"
      ? target
      : currentTool
        ? humanizeToolName(currentTool.name)
        : null,
  );

  // Live token estimate for whatever is currently streaming: thinking text
  // or tool call argument deltas. Null when nothing countable is streaming.
  const streamTokens = $derived.by(() => {
    const lastMsg = messages.at(-1);
    if (lastMsg?.type !== "assistant") return null;

    if (session.streaming_tool_name) {
      const args = lastMsg.tool_calls?.at(-1)?.arguments;
      if (!args) return null;
      const tokens = estimateJsonTokens(args);
      return tokens > 0 ? formatStreamTokens(tokens) : null;
    }

    if (status !== "Thinking") return null;
    const lastBlock = lastMsg.content.at(-1);
    if (lastBlock?.type !== "thinking" || !lastBlock.thinking) return null;
    const tokens = estimateTextTokens(lastBlock.thinking);
    return tokens > 0 ? formatStreamTokens(tokens) : null;
  });

  const visible = $derived(isActiveSessionPhase(session.phase));

  // Elapsed time of the current run. Run boundaries are tracked in
  // session-phase (idle→active transitions), so the clock resets on every
  // new run, survives switching sessions mid-run, and needs no cleanup
  // here — this component only ticks the display.
  let now = $state(Date.now());
  const runStart = $derived(visible ? noteRunStart(session.id) : null);
  const elapsed = $derived(
    runStart != null ? Math.floor((now - runStart) / 1000) : 0,
  );

  $effect(() => {
    if (!visible) return;
    const timer = setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => clearInterval(timer);
  });
</script>

{#if visible}
  <div
    class="inline-flex min-h-5 -mt-1.5 items-center gap-2 pl-0.5 text-[12px] font-normal text-muted-foreground/75"
    role="status"
    aria-live="polite"
    aria-atomic="true"
  >
    <span>
      <span class="status-shimmer" data-text={leadWord}>{leadWord}</span
      >{#if accent}
        <span
          class="ml-1 inline-block max-w-72 truncate align-bottom font-mono font-medium text-primary/85"
          title={accent}>{accent}</span
        >
      {/if}<span class="status-shimmer" data-text="...">...</span>
    </span>
    <!-- Elapsed and token counts tick constantly; keep them out of the
         aria-live region so screen readers are not bombarded. -->
    <span
      aria-hidden="true"
      class="font-mono tabular-nums text-muted-foreground/60"
      >{formatRunElapsed(elapsed)}</span
    >
    {#if streamTokens}
      <span
        aria-hidden="true"
        class="font-mono tabular-nums text-muted-foreground/60"
        >{streamTokens}</span
      >
    {/if}
  </div>
{/if}

<style>
  /* Shimmer sweep across the status word: muted base, foreground peak. */
  .status-shimmer {
    position: relative;
    font-style: italic;
    background: linear-gradient(
      90deg,
      var(--color-muted-foreground) 0%,
      var(--color-muted-foreground) 30%,
      var(--color-foreground) 50%,
      var(--color-muted-foreground) 70%,
      var(--color-muted-foreground) 100%
    );
    background-size: 200% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    -webkit-text-fill-color: transparent;
    animation: shimmer-sweep 1.6s linear infinite;
  }

  /* Halo copy layered on top: only the peak band is opaque, so the glow
     exists solely where the sweep currently is. Blur turns the clipped
     glyphs into a bloom. Runs the same keyframes to stay in sync. */
  .status-shimmer::before {
    content: attr(data-text);
    position: absolute;
    inset: 0;
    pointer-events: none;
    background: linear-gradient(
      90deg,
      transparent 0%,
      transparent 34%,
      hsl(var(--primary) / 0.3) 50%,
      transparent 66%,
      transparent 100%
    );
    background-size: 200% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    -webkit-text-fill-color: transparent;
    filter: blur(3.5px);
    animation: shimmer-sweep 1.6s linear infinite;
  }

  @keyframes shimmer-sweep {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -100% 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .status-shimmer {
      animation: none;
      background: none;
      color: inherit;
      -webkit-text-fill-color: currentColor;
    }
    .status-shimmer::before {
      content: none;
    }
  }
</style>
