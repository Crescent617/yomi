<script lang="ts">
  import { flushSync, untrack } from "svelte";
  import type { Message, SessionState } from "../../state.svelte";
  import { findThinking, hasText } from "../../session";
  import { isActiveSessionPhase, noteRunStart } from "../../session-phase";
  import { humanizeToolName } from "../tool/tool-utils";
  import { estimateStreamTokens } from "../../tokens";
  import {
    extractPartialTarget,
    formatStreamTokens,
    formatTapeElapsed,
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
  const verb = $derived(
    currentTool ? toolVerb(currentTool.name, currentTool.args) : null,
  );
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

  // Run-cumulative output tokens: real completions folded at each response
  // end, the in-flight estimate (or pending usage report) on top.
  const streamTokens = $derived.by(() => {
    const s = session.out_stream;
    if (!s) return null;
    const total = s.run + (s.pending ?? estimateStreamTokens(s.text, s.json));
    return total > 0 ? formatStreamTokens(total) : null;
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

  // ── Lead-word swap ──
  // The displayed word lags `leadWord`: the old word rises and dissolves
  // (blur + fade), the text swaps at the hidden point, and the new word
  // settles in from below. Reduced motion swaps instantly.
  const SWAP_OUT_MS = 140;
  // Starts at the current lead word, then lags behind it via the swap effect.
  let displayWord = $state(untrack(() => leadWord));
  let swapOut = $state(false);
  let swapReset = $state(false);
  let verbRef = $state<HTMLSpanElement | null>(null);
  let swapTimer: ReturnType<typeof setTimeout> | null = null;
  const reduceMotion =
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  $effect(() => {
    const next = leadWord;
    untrack(() => {
      if (next === displayWord) {
        // Bounced back to the displayed word mid-swap: settle instead of
        // staying parked in the (invisible) out pose.
        swapOut = false;
        return;
      }
      if (reduceMotion) {
        displayWord = next;
        return;
      }
      swapOut = true;
      if (swapTimer) clearTimeout(swapTimer);
      swapTimer = setTimeout(() => {
        swapTimer = null;
        displayWord = next;
        swapOut = false;
        // Jump to the entry pose (below, dissolved) without transitioning,
        // then release so the word animates up into place.
        swapReset = true;
        flushSync();
        void verbRef?.offsetWidth;
        swapReset = false;
      }, SWAP_OUT_MS);
    });
    return () => {
      if (swapTimer) {
        clearTimeout(swapTimer);
        swapTimer = null;
      }
    };
  });
</script>

{#if visible}
  <div
    class="relative pl-0.5 pr-1 text-[12px] font-normal text-muted-foreground/75"
    role="status"
    aria-live="polite"
    aria-atomic="true"
  >
    <div class="flex min-h-5 items-center gap-1.5">
      <span
        bind:this={verbRef}
        class="status-shimmer lead-word shrink-0 font-mono text-sm"
        class:swap-out={swapOut}
        class:swap-reset={swapReset}
        data-text={displayWord}>{displayWord}</span
      >
      {#if accent}
        <span
          class="min-w-0 truncate font-mono font-medium text-primary/85"
          title={accent}>{accent}</span
        >
      {/if}
      <span class="flex-1"></span>
      <!-- Telemetry ticks constantly; kept out of the aria-live
           region so screen readers are not bombarded. -->
      {#if streamTokens}
        <span
          aria-hidden="true"
          class="shrink-0 font-mono tabular-nums text-muted-foreground/60"
          >{streamTokens}</span
        >
      {/if}
      <span
        aria-hidden="true"
        class="shrink-0 font-mono tabular-nums text-muted-foreground/60"
        >{formatTapeElapsed(elapsed)}</span
      >
    </div>
    <!-- Signature scan line: the single motion accent of the status row. -->
    <span class="tape-scan" aria-hidden="true"></span>
  </div>
{/if}

<style>
  /* Lead-word swap: quick rise-and-dissolve roll between status verbs. */
  .lead-word {
    transition:
      transform 200ms cubic-bezier(0.16, 1, 0.3, 1),
      opacity 160ms ease-out,
      filter 200ms ease-out;
  }

  .lead-word.swap-out {
    transform: translateY(-0.3em);
    opacity: 0;
    filter: blur(3px);
    transition:
      transform 140ms cubic-bezier(0.5, 0, 0.75, 0),
      opacity 120ms ease-in,
      filter 140ms ease-in;
  }

  /* Entry pose: applied for one forced reflow so the settle-in transition
     starts from below. */
  .lead-word.swap-reset {
    transition: none;
    transform: translateY(0.3em);
    opacity: 0;
    filter: blur(3px);
  }

  @media (prefers-reduced-motion: reduce) {
    .lead-word {
      transition: none;
    }
  }

  /* 1px theme-color gradient sweeping the underside of the status row. */
  .tape-scan {
    position: absolute;
    bottom: -2px;
    left: 0;
    right: 0;
    height: 1px;
    overflow: hidden;
  }

  .tape-scan::before {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    width: 40%;
    background: linear-gradient(
      90deg,
      transparent,
      hsl(var(--primary) / 0.55),
      transparent
    );
    animation: tape-scan-sweep 2.4s linear infinite;
  }

  @keyframes tape-scan-sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .tape-scan::before {
      animation: none;
      transform: translateX(80%);
      opacity: 0.5;
    }
  }

  /* Shimmer sweep across the status word: muted base, foreground peak. */
  .status-shimmer {
    position: relative;
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
    animation: shimmer-sweep 2.4s linear infinite;
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
    animation: shimmer-sweep 2.4s linear infinite;
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
