<script lang="ts">
  import {
    FileText,
    FilePenLine,
    FileEdit,
    FolderSearch,
    Search,
    Terminal,
    Moon,
    Bell,
    Globe,
    Bot,
    MessageSquare,
    ListChecks,
    Target,
    Send,
    Wrench,
  } from "lucide-svelte";

  let {
    toolName,
    isRunning = false,
    className = "",
  }: {
    toolName: string;
    isRunning?: boolean;
    className?: string;
  } = $props();

  const cls = $derived(
    `${className} ${isRunning ? "animate-pulse" : ""}`.trim(),
  );

  const name = $derived(toolName.toLowerCase());
</script>

{#if name === "read" || name === "read_file"}
  <FileText class={cls} />
{:else if name === "write" || name === "write_file"}
  <FilePenLine class={cls} />
{:else if name === "edit" || name === "edit_file"}
  <FileEdit class={cls} />
{:else if name === "glob" || name === "glob_search"}
  <FolderSearch class={cls} />
{:else if name === "grep" || name === "grep_search"}
  <Search class={cls} />
{:else if name === "bash" || name === "shell" || name === "command"}
  <Terminal class={cls} />
{:else if name === "sleep"}
  <Moon class={cls} />
{:else if name === "reminder"}
  <Bell class={cls} />
{:else if name === "webfetch" || name === "web_fetch"}
  <Globe class={cls} />
{:else if name === "websearch" || name === "web_search"}
  <Globe class={cls} />
{:else if name === "agent" || name === "subagent" || name === "sub_agent"}
  <Bot class={cls} />
{:else if name === "ask_user" || name === "askuser" || name === "ask"}
  <MessageSquare class={cls} />
{:else if name === "todo" || name === "task"}
  <ListChecks class={cls} />
{:else if name.includes("goal")}
  <Target class={cls} />
{:else if name === "send_message" || name === "post_message" || name === "message"}
  <Send class={cls} />
{:else}
  <Wrench class={cls} />
{/if}
