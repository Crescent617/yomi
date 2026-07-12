<script lang="ts">
  import { Loader2 } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";
  import {
    diffLines,
    parseEditArgs,
    parsePostMessageArgs,
    parseWriteArgs,
  } from "./tool-utils";

  let { tool, embedded = false }: { tool: ToolCall; embedded?: boolean } =
    $props();

  const editArgs = $derived(parseEditArgs(tool.arguments ?? ""));
  const writeArgs = $derived(parseWriteArgs(tool.arguments ?? ""));
  const postMessageArgs = $derived(
    tool.tool_name === "post_message"
      ? parsePostMessageArgs(tool.arguments ?? "")
      : null,
  );
</script>

<div
  class="space-y-1.5 max-h-96 overflow-y-auto {embedded
    ? 'py-1 pr-1'
    : 'border-t border-subtle px-3 pb-2'}"
>
  <!-- Edit tool: diff view -->
  {#if editArgs}
    <div class="text-xs">
      {#if !embedded}
        <div class="font-medium mb-1 opacity-60 flex items-center gap-1.5">
          <span class="font-mono">{editArgs.path}</span>
        </div>
      {/if}
      <div
        class="rounded border border-subtle bg-code-bg overflow-hidden font-mono text-[11px] leading-relaxed"
      >
        {#each diffLines(editArgs.old_str, editArgs.new_str) as line, i (i)}
          <div
            class="flex
            {line.type === 'add' ? 'bg-success/10' : ''}
            {line.type === 'del' ? 'bg-error/10' : ''}"
          >
            <span
              class="shrink-0 w-5 text-right pr-1 select-none
              {line.type === 'add' ? 'text-success' : ''}
              {line.type === 'del' ? 'text-error' : ''}
              {line.type === 'context' ? 'text-muted-foreground' : ''}"
            >
              {line.type === "add" ? "+" : line.type === "del" ? "−" : " "}
            </span>
            <span
              class="whitespace-pre-wrap flex-1 min-w-0
              {line.type === 'add' ? 'text-success' : ''}
              {line.type === 'del' ? 'text-error' : ''}
              {line.type === 'context' ? 'text-foreground/80' : ''}"
            >
              {line.text}
            </span>
          </div>
        {/each}
      </div>
    </div>

    <!-- Write tool: content view -->
  {:else if writeArgs}
    <div class="text-xs">
      {#if !embedded}
        <div class="font-medium mb-1 opacity-60 flex items-center gap-1.5">
          <span class="font-mono">{writeArgs.file_path}</span>
        </div>
      {/if}
      <pre
        class="bg-code-bg rounded px-2.5 py-2 whitespace-pre-wrap overflow-x-auto text-[11px] leading-relaxed font-mono text-foreground/80">{writeArgs.content}</pre>
    </div>

    <!-- Post message tool: recipient, subject, and body -->
  {:else if postMessageArgs}
    <div class="rounded border border-info/20 bg-info/5 px-2.5 py-2 text-xs">
      <div class="flex items-baseline gap-2">
        <span
          class="shrink-0 text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
          >To</span
        >
        <span class="truncate font-mono text-[11px] text-info"
          >{postMessageArgs.agent_id}</span
        >
      </div>
      <div class="mt-1 font-medium text-foreground">
        {postMessageArgs.title}
      </div>
      <div class="mt-1 whitespace-pre-wrap break-words text-foreground/80">
        {postMessageArgs.content}
      </div>
    </div>

    <!-- Other tools: raw JSON -->
  {:else if tool.arguments}
    <div class="text-xs opacity-60">
      <div class="font-medium mb-0.5">Arguments:</div>
      <pre
        class="bg-code-bg rounded px-2 py-1 whitespace-pre-wrap">{tool.arguments}</pre>
    </div>
  {/if}

  {#if tool.status === "running"}
    <div class="text-xs italic opacity-50 flex items-center gap-1">
      <Loader2 class="w-3 h-3 animate-spin" /> Running…
      {#if tool.progress}<span>{tool.progress}</span>{/if}
    </div>
  {/if}

  {#if tool.output}
    <div class="text-xs">
      <div class="font-medium mb-0.5 opacity-60">Output:</div>
      <pre
        class="bg-code-bg rounded px-2 py-1 whitespace-pre-wrap overflow-x-auto">{tool.output}</pre>
    </div>
  {/if}

  {#if tool.error}
    <div class="text-xs text-error">
      <div class="font-medium mb-0.5">Error:</div>
      <pre
        class="bg-error/10 rounded px-2 py-1 whitespace-pre-wrap">{tool.error}</pre>
    </div>
  {/if}
</div>
