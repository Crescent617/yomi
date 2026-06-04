<script lang="ts">
  import { ChevronDown, ChevronUp, Loader2, CheckCircle2, XCircle, MinusCircle, AlertCircle } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";
  import { formatElapsed } from "../../utils";

  let { tool }: { tool: ToolCall } = $props();

  let expanded = $state(false);

  function statusColor(status: string): string {
    switch (status) {
      case "running":
        return "text-amber-700 border-amber-200 bg-amber-50/60 dark:text-amber-400 dark:border-amber-800 dark:bg-amber-950/30";
      case "completed":
        return "text-green-700 border-green-200 bg-green-50/60 dark:text-green-400 dark:border-green-800 dark:bg-green-950/30";
      case "failed":
        return "text-red-700 border-red-200 bg-red-50/60 dark:text-red-400 dark:border-red-800 dark:bg-red-950/30";
      case "cancelled":
        return "text-gray-600 border-gray-200 bg-gray-50/60 dark:text-gray-400 dark:border-gray-700 dark:bg-gray-900/50";
      default:
        return "text-gray-600 border-gray-200 bg-gray-50/60 dark:text-gray-400 dark:border-gray-700 dark:bg-gray-900/50";
    }
  }

  function extractTarget(toolName: string, args: string): string {
    if (!args) return "";
    try {
      const parsed = JSON.parse(args);
      switch (toolName.toLowerCase()) {
        case "read": case "edit": return parsed.path ?? "";
        case "write": return parsed.file_path ?? "";
        case "shell": return parsed.command ?? "";
        case "glob": case "grep": return parsed.pattern ?? "";
        case "webfetch": return parsed.url ?? "";
        case "skill": return parsed.name ?? parsed.path ?? "";
        case "subagent": return parsed.description ?? "";
        default: return "";
      }
    } catch { return ""; }
  }

  function compactArgs(args: string, maxLen = 120): string {
    if (!args) return "";
    try {
      const parsed = JSON.parse(args);
      const s = JSON.stringify(parsed);
      if (s.length <= maxLen) return s;
      return s.slice(0, maxLen) + "…";
    } catch {
      return args.replace(/\s+/g, " ").slice(0, maxLen) + (args.length > maxLen ? "…" : "");
    }
  }

  function extraMeta(toolName: string, args: string): string {
    if (!args) return "";
    try {
      const parsed = JSON.parse(args);
      const extras: string[] = [];
      switch (toolName.toLowerCase()) {
        case "shell": {
          if (parsed.background) extras.push("async");
          const timeout = parsed.timeout;
          if (timeout != null && (parsed.background || timeout !== 60)) {
            extras.push(`timeout ${timeout}s`);
          }
          break;
        }
        case "grep": {
          const mode = parsed.output_mode || "filename";
          if (mode !== "filename") extras.push(mode);
          break;
        }
        case "subagent": {
          const preset = parsed.preset || "general-purpose";
          if (preset !== "general-purpose") extras.push(preset);
          break;
        }
      }
      return extras.join(" · ");
    } catch { return ""; }
  }

  const target = $derived(extractTarget(tool.toolName, tool.arguments ?? ""));
  const meta = $derived(extraMeta(tool.toolName, tool.arguments ?? ""));
</script>

<div class="rounded-md border text-sm overflow-hidden {statusColor(tool.status)}">
  <!-- Header — always visible: name + target/args + status + chevron -->
  <button
    type="button"
    class="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-black/5 dark:hover:bg-white/5 transition-colors"
    onclick={() => expanded = !expanded}
  >
    {#if tool.status === "running"}
      <Loader2 class="w-4 h-4 shrink-0 animate-spin" />
    {:else if tool.status === "completed"}
      <CheckCircle2 class="w-4 h-4 shrink-0" />
    {:else if tool.status === "failed"}
      <XCircle class="w-4 h-4 shrink-0" />
    {:else if tool.status === "cancelled"}
      <MinusCircle class="w-4 h-4 shrink-0" />
    {:else}
      <AlertCircle class="w-4 h-4 shrink-0" />
    {/if}

    <span class="font-semibold capitalize shrink-0">{tool.toolName}</span>

    {#if target}
      <span class="text-xs opacity-70 truncate">{target}</span>
    {:else if tool.arguments}
      <span class="text-xs opacity-60 truncate">{compactArgs(tool.arguments, 80)}</span>
    {/if}
    {#if meta}
      <span class="text-xs opacity-50 shrink-0">· {meta}</span>
    {/if}

    {#if tool.elapsedMs && tool.elapsedMs > 1000}
      <span class="text-xs opacity-60 shrink-0">{formatElapsed(tool.elapsedMs)}</span>
    {/if}
    {#if tool.progress && tool.status === "running"}
      <span class="text-xs opacity-60 truncate">· {tool.progress}</span>
    {/if}
    {#if tool.tokens}
      <span class="text-xs opacity-60 shrink-0">· {tool.tokens} tokens</span>
    {/if}
    <span class="ml-auto">
      {#if expanded}
        <ChevronUp class="w-3.5 h-3.5 opacity-50" />
      {:else}
        <ChevronDown class="w-3.5 h-3.5 opacity-50" />
      {/if}
    </span>
  </button>

  <!-- Expanded body — args, output, error -->
  {#if expanded}
    <div class="px-3 pb-2 space-y-1.5 border-t border-black/5 dark:border-white/10">
      {#if tool.arguments}
        <div class="text-xs opacity-60 dark:opacity-50">
          <div class="font-medium mb-0.5">Arguments:</div>
          <pre class="bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap">{compactArgs(tool.arguments)}</pre>
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
          <div class="font-medium mb-0.5 opacity-70 dark:opacity-50">Output:</div>
          <pre class="bg-black/5 dark:bg-white/5 rounded px-2 py-1 whitespace-pre-wrap overflow-x-auto">{tool.output}</pre>
        </div>
      {/if}

      {#if tool.error}
        <div class="text-xs text-red-600 dark:text-red-400">
          <div class="font-medium mb-0.5">Error:</div>
          <pre class="bg-red-50/80 dark:bg-red-950/40 rounded px-2 py-1 whitespace-pre-wrap">{tool.error}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>
