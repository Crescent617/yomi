<script lang="ts">
  import { Loader2, FileEdit, FileText } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";
  import { parseEditArgs, parseWriteArgs, diffLines } from "./tool-utils";

  let { tool }: { tool: ToolCall } = $props();

  const editArgs = $derived(parseEditArgs(tool.arguments ?? ""));
  const writeArgs = $derived(parseWriteArgs(tool.arguments ?? ""));
</script>

<div
  class="px-3 pb-2 space-y-1.5 border-t border-black/5 dark:border-white/10 max-h-96 overflow-y-auto"
>
  <!-- Edit tool: diff view -->
  {#if editArgs}
    <div class="text-xs">
      <div class="font-medium mb-1 opacity-70 dark:opacity-50 flex items-center gap-1.5">
        <FileEdit class="w-3.5 h-3.5" />
        <span class="font-mono">{editArgs.path}</span>
      </div>
      <div class="rounded border border-black/5 dark:border-white/10 overflow-hidden font-mono text-[11px] leading-relaxed">
        {#each diffLines(editArgs.old_str, editArgs.new_str) as line, i (i)}
          <div class="flex
            {line.type === 'add' ? 'bg-green-50/60 dark:bg-green-950/20' : ''}
            {line.type === 'del' ? 'bg-red-50/60 dark:bg-red-950/20' : ''}"
          >
            <span class="shrink-0 w-5 text-right pr-1 select-none
              {line.type === 'add' ? 'text-green-600 dark:text-green-400' : ''}
              {line.type === 'del' ? 'text-red-600 dark:text-red-400' : ''}
              {line.type === 'context' ? 'text-gray-400 dark:text-gray-500' : ''}"
            >
              {line.type === 'add' ? '+' : line.type === 'del' ? '−' : ' '}
            </span>
            <span class="whitespace-pre-wrap flex-1 min-w-0
              {line.type === 'add' ? 'text-green-700 dark:text-green-300' : ''}
              {line.type === 'del' ? 'text-red-700 dark:text-red-300' : ''}
              {line.type === 'context' ? 'text-foreground/80 dark:text-foreground/70' : ''}"
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
      <div class="font-medium mb-1 opacity-70 dark:opacity-50 flex items-center gap-1.5">
        <FileText class="w-3.5 h-3.5" />
        <span class="font-mono">{writeArgs.file_path}</span>
      </div>
      <pre
        class="bg-black/5 dark:bg-white/5 rounded px-2.5 py-2 whitespace-pre-wrap overflow-x-auto text-[11px] leading-relaxed font-mono text-foreground/90 dark:text-foreground/80">{writeArgs.content}</pre>
    </div>

  <!-- Other tools: raw JSON -->
  {:else if tool.arguments}
    <div class="text-xs opacity-60 dark:opacity-50">
      <div class="font-medium mb-0.5">Arguments:</div>
      <pre
        class="bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap">{tool.arguments}</pre>
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
      <div class="font-medium mb-0.5 opacity-70 dark:opacity-50">
        Output:
      </div>
      <pre
        class="bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap overflow-x-auto">{tool.output}</pre>
    </div>
  {/if}

  {#if tool.error}
    <div class="text-xs text-red-600 dark:text-red-400">
      <div class="font-medium mb-0.5">Error:</div>
      <pre
        class="bg-red-50/80 dark:bg-red-950/40 rounded px-2 py-1 whitespace-pre-wrap">{tool.error}</pre>
    </div>
  {/if}
</div>
