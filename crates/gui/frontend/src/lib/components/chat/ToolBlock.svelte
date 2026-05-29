<script lang="ts">
  import { ChevronDown, ChevronUp, Loader2, CheckCircle2, XCircle, MinusCircle, AlertCircle } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";

  let { tool }: { tool: ToolCall } = $props();

  let expanded = $state(false);

  function toolIcon(name: string): string {
    const map: Record<string, string> = {
      shell: "", read: "", write: "", edit: "", grep: "󰑑",
      glob: "󰱼", webfetch: "󰖟", websearch: "", subagent: "󰚩",
      skill: "⚡", reminder: "󰀠", todo: "", ask_user: "",
    };
    return map[name.toLowerCase()] ?? "";
  }

  function statusColor(status: string): string {
    switch (status) {
      case "running": return "text-amber-600 border-amber-200 bg-amber-50/50";
      case "completed": return "text-green-600 border-green-200 bg-green-50/50";
      case "failed": return "text-red-600 border-red-200 bg-red-50/50";
      case "cancelled": return "text-gray-500 border-gray-200 bg-gray-50/50";
      default: return "text-gray-500 border-gray-200 bg-gray-50/50";
    }
  }

  function formatElapsed(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  function extractTarget(toolName: string, args: string): string {
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
    try {
      const parsed = JSON.parse(args);
      const s = JSON.stringify(parsed);
      if (s.length <= maxLen) return s;
      return s.slice(0, maxLen) + "…";
    } catch {
      return args.replace(/\s+/g, " ").slice(0, maxLen) + (args.length > maxLen ? "…" : "");
    }
  }
</script>

<div class="rounded-md border text-sm overflow-hidden {statusColor(tool.status)}">
  <!-- Header — always visible, shows name + icon + status + chevron -->
  <button
    type="button"
    class="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-black/3 dark:hover:bg-white/3 transition-colors"
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
    <span class="font-mono">{toolIcon(tool.toolName)}</span>
    <span class="font-semibold capitalize">{tool.toolName}</span>
    {#if tool.elapsedMs && tool.elapsedMs > 1000}
      <span class="text-xs opacity-60">{formatElapsed(tool.elapsedMs)}</span>
    {/if}
    {#if tool.progress && tool.status === "running"}
      <span class="text-xs opacity-60 truncate">· {tool.progress}</span>
    {/if}
    {#if tool.tokens}
      <span class="text-xs opacity-60">· {tool.tokens} tokens</span>
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
    <div class="px-3 pb-2 space-y-1.5 border-t border-black/5 dark:border-white/5">
      {#if tool.arguments}
        {@const target = extractTarget(tool.toolName, tool.arguments)}
        {#if target}
          <div class="text-xs pt-1.5 font-medium opacity-70">{target}</div>
        {/if}
        <div class="text-xs opacity-60">
          <div class="font-medium mb-0.5">Arguments:</div>
          <pre class="bg-black/3 dark:bg-white/3 rounded px-2 py-1 overflow-x-auto">{compactArgs(tool.arguments)}</pre>
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
          <div class="font-medium mb-0.5 opacity-70">Output:</div>
          <pre class="bg-black/3 dark:bg-white/3 rounded px-2 py-1 whitespace-pre-wrap overflow-x-auto">{tool.output}</pre>
        </div>
      {/if}

      {#if tool.error}
        <div class="text-xs text-red-600">
          <div class="font-medium mb-0.5">Error:</div>
          <pre class="bg-red-50 dark:bg-red-950/30 rounded px-2 py-1 whitespace-pre-wrap">{tool.error}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>
