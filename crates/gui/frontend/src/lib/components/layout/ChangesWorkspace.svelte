<script lang="ts">
  import { onMount } from "svelte";
  import {
    RefreshCw,
    FilePlus,
    FileMinus,
    FileEdit,
    PanelLeftOpen,
    PanelLeftClose,
    Loader,
    ArrowLeft,
    ArrowUp,
    ArrowDown,
    AlertCircle,
    Check,
  } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import * as api from "../../api";

  let {
    session,
    onClose,
  }: { session: SessionState | null; onClose?: () => void } = $props();

  interface FileSummary {
    path: string;
    status: string;
  }

  interface DiffFile {
    oldPath?: string;
    newPath?: string;
    hunks: DiffHunk[];
  }

  interface DiffHunk {
    old_start: number;
    old_lines: number;
    new_start: number;
    new_lines: number;
    header: string;
    lines: DiffLine[];
  }

  interface DiffLine {
    type: "context" | "add" | "del" | "hunk";
    oldLine: number | null;
    newLine: number | null;
    text: string;
  }

  let files = $state<FileSummary[]>([]);
  let workingFiles = $state<FileSummary[]>([]);
  let stagedFiles = $state<FileSummary[]>([]);
  let showStaged = $state(false);
  let loading = $state(false);
  let loadError = $state("");
  let loadingFile = $state<string | null>(null);
  let diffError = $state("");
  let activeFilePath = $state<string | null>(null);
  let viewMode = $state<"unified" | "split">("unified");
  let diffFiles = $state<DiffFile[]>([]);
  let loadVersion = 0;
  let diffLoadVersion = 0;
  let lastPath = "";
  let lastStaged = false;
  let lastWorkingFile: string | null = null;
  let lastStagedFile: string | null = null;
  let _lastRawDiff = "";
  let showFileTree = $state(true);

  const currentFileIndex = $derived(
    activeFilePath
      ? files.findIndex((file) => file.path === activeFilePath)
      : -1,
  );

  function fileName(path: string) {
    return path.split("/").pop() || path;
  }

  function parentPath(path: string) {
    const parts = path.split("/").slice(0, -1);
    if (parts.length === 0) return "Repository root";
    return parts.length > 2
      ? `…/${parts.slice(-2).join("/")}`
      : parts.join("/");
  }

  function parseDiff(raw: string): DiffFile[] {
    const files: DiffFile[] = [];
    let currentFile: DiffFile | null = null;
    let currentHunk: DiffHunk | null = null;
    let oldLine = 0;
    let newLine = 0;

    const lines = raw.split("\n");
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];

      if (line.startsWith("diff --git")) {
        if (currentHunk && currentFile) currentFile.hunks.push(currentHunk);
        if (currentFile) files.push(currentFile);
        currentFile = { oldPath: undefined, newPath: undefined, hunks: [] };
        currentHunk = null;
        i++;
        continue;
      }

      if (!currentFile) {
        i++;
        continue;
      }

      if (line.startsWith("--- ")) {
        currentFile.oldPath = line.slice(4).replace(/^a\//, "");
        i++;
        continue;
      }
      if (line.startsWith("+++ ")) {
        currentFile.newPath = line.slice(4).replace(/^b\//, "");
        i++;
        continue;
      }

      if (line.startsWith("@@")) {
        if (currentHunk) currentFile.hunks.push(currentHunk);
        const match = line.match(/^@@ -(\d+),?(\d*) \+(\d+),?(\d*) @@(.*)/);
        if (match) {
          const old_start = parseInt(match[1], 10);
          const old_lines = match[2] ? parseInt(match[2], 10) : 1;
          const new_start = parseInt(match[3], 10);
          const new_lines = match[4] ? parseInt(match[4], 10) : 1;
          currentHunk = {
            old_start,
            old_lines,
            new_start,
            new_lines,
            header: match[5] || "",
            lines: [],
          };
          oldLine = old_start;
          newLine = new_start;
        }
        i++;
        continue;
      }

      if (currentHunk) {
        if (line.length === 0) {
          currentHunk.lines.push({
            type: "context",
            oldLine,
            newLine,
            text: "",
          });
          oldLine++;
          newLine++;
        } else {
          const ch = line[0];
          const text = line.slice(1);
          if (ch === " ") {
            currentHunk.lines.push({ type: "context", oldLine, newLine, text });
            oldLine++;
            newLine++;
          } else if (ch === "+") {
            currentHunk.lines.push({
              type: "add",
              oldLine: null,
              newLine,
              text,
            });
            newLine++;
          } else if (ch === "-") {
            currentHunk.lines.push({
              type: "del",
              oldLine,
              newLine: null,
              text,
            });
            oldLine++;
          } else if (ch === "\\") {
            currentHunk.lines.push({
              type: "context",
              oldLine,
              newLine,
              text: line,
            });
          } else {
            currentHunk.lines.push({
              type: "context",
              oldLine,
              newLine,
              text: line,
            });
            oldLine++;
            newLine++;
          }
        }
      }

      i++;
    }

    if (currentHunk && currentFile) currentFile.hunks.push(currentHunk);
    if (currentFile) files.push(currentFile);

    return files.filter((f) => f.hunks.length > 0);
  }

  async function loadFileList(options: { preserveSelection?: boolean } = {}) {
    const path = session?.project_path;
    if (!path) return;
    const staged = showStaged;
    const currentVersion = ++loadVersion;
    const previousFile = activeFilePath;

    loading = true;
    loadError = "";
    try {
      const [workingResult, stagedResult] = await Promise.all([
        api.getGitDiffSummary(path, false),
        api.getGitDiffSummary(path, true),
      ]);
      if (currentVersion !== loadVersion) return;
      workingFiles = workingResult ?? [];
      stagedFiles = stagedResult ?? [];
      files = staged ? stagedFiles : workingFiles;
      const rememberedFile = staged ? lastStagedFile : lastWorkingFile;
      const preferredFile = options.preserveSelection
        ? previousFile
        : rememberedFile;
      const nextFile =
        preferredFile && files.some((file) => file.path === preferredFile)
          ? preferredFile
          : (files[0]?.path ?? null);
      activeFilePath = null;
      diffFiles = [];
      _lastRawDiff = "";
      if (nextFile) void loadFileDiff(nextFile);
    } catch (e) {
      console.error("Failed to load git diff summary:", e);
      files = [];
      activeFilePath = null;
      diffFiles = [];
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      if (currentVersion === loadVersion) {
        loading = false;
      }
    }
  }

  async function loadFileDiff(filePath: string) {
    const path = session?.project_path;
    if (!path) return;

    const idx = files.findIndex((f) => f.path === filePath);
    if (idx < 0) return;

    activeFilePath = filePath;
    if (showStaged) lastStagedFile = filePath;
    else lastWorkingFile = filePath;

    const currentStaged = showStaged;
    const currentDiffVersion = ++diffLoadVersion;
    loadingFile = filePath;
    diffError = "";

    try {
      const raw = await api.getGitFileDiffRaw(path, filePath, currentStaged);
      if (
        currentDiffVersion !== diffLoadVersion ||
        showStaged !== currentStaged
      )
        return;
      if (
        files.findIndex((f) => f.path === filePath) < 0 ||
        activeFilePath !== filePath
      )
        return;

      if (!raw) {
        diffFiles = [];
        _lastRawDiff = "";
        return;
      }

      _lastRawDiff = raw;
      diffFiles = parseDiff(raw);
    } catch (e) {
      console.error("Failed to load file diff:", e);
      diffFiles = [];
      _lastRawDiff = "";
      diffError = e instanceof Error ? e.message : String(e);
    } finally {
      if (loadingFile === filePath) loadingFile = null;
    }
  }

  function maybeLoad() {
    const path = session?.project_path;
    if (!path) {
      files = [];
      diffFiles = [];
      _lastRawDiff = "";
      return;
    }
    if (path === lastPath && showStaged === lastStaged) return;
    lastPath = path;
    lastStaged = showStaged;
    void loadFileList({ preserveSelection: true });
  }

  function refresh() {
    const path = session?.project_path;
    if (!path) return;
    lastPath = path;
    lastStaged = showStaged;
    void loadFileList({ preserveSelection: true });
  }

  function selectStaged(staged: boolean) {
    if (showStaged === staged) return;
    showStaged = staged;
    lastStaged = staged;
    void loadFileList();
  }

  function selectRelativeFile(delta: number) {
    if (currentFileIndex < 0) return;
    const next = files[currentFileIndex + delta];
    if (next) void loadFileDiff(next.path);
  }

  function onWorkspaceKeydown(event: KeyboardEvent) {
    if (!event.altKey) return;
    if (event.key === "ArrowUp") {
      event.preventDefault();
      selectRelativeFile(-1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      selectRelativeFile(1);
    }
  }

  onMount(() => {
    maybeLoad();
  });

  $effect(() => {
    const path = session?.project_path;
    if (path && path !== lastPath) {
      maybeLoad();
    }
  });

  function lineBg(type: DiffLine["type"]) {
    switch (type) {
      case "add":
        return "bg-success/8";
      case "del":
        return "bg-error/8";
      case "hunk":
        return "bg-primary/6 text-primary";
      default:
        return "hover:bg-secondary/20";
    }
  }

  function lineText(type: DiffLine["type"]) {
    return type === "hunk" ? "text-primary" : "text-foreground";
  }

  function lineNumberBg(type: DiffLine["type"]) {
    if (type === "add") return "bg-success/12";
    if (type === "del") return "bg-error/12";
    return "";
  }

  function leftLineBg(type: DiffLine["type"]) {
    // left column: old file — del lines are red, add lines are blank placeholder
    if (type === "del") return "bg-error/8";
    if (type === "add") return "bg-secondary/20";
    return "";
  }

  function leftLineText(type: DiffLine["type"]) {
    if (type === "del") return "text-foreground";
    if (type === "add") return "text-muted-foreground";
    return "text-foreground";
  }

  function rightLineBg(type: DiffLine["type"]) {
    // right column: new file — add lines are green, del lines are blank placeholder
    if (type === "add") return "bg-success/8";
    if (type === "del") return "bg-secondary/20";
    return "";
  }

  function rightLineText(type: DiffLine["type"]) {
    if (type === "add") return "text-foreground";
    if (type === "del") return "text-muted-foreground";
    return "text-foreground";
  }
</script>

<svelte:window onkeydown={onWorkspaceKeydown} />

<div class="flex flex-col h-full bg-background min-w-0">
  <!-- Workspace controls -->
  <div
    class="flex h-11 shrink-0 items-center justify-between gap-3 border-b border-border/70 px-3"
  >
    <div class="flex min-w-0 items-center gap-2">
      {#if onClose}
        <button
          type="button"
          onclick={onClose}
          class="inline-flex h-7 items-center gap-1.5 rounded-md border border-border bg-secondary/70 px-2.5 text-xs font-medium text-foreground shadow-sm transition-colors hover:bg-secondary"
          aria-label="Back to chat"
        >
          <ArrowLeft size={14} />
          Chat
        </button>
      {/if}
      <button
        type="button"
        onclick={() => (showFileTree = !showFileTree)}
        class="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        aria-label={showFileTree
          ? "Collapse changed files"
          : "Expand changed files"}
        aria-pressed={showFileTree}
        title={showFileTree ? "Collapse changed files" : "Expand changed files"}
      >
        {#if showFileTree}
          <PanelLeftClose size={15} />
        {:else}
          <PanelLeftOpen size={15} />
        {/if}
      </button>
      <div class="inline-flex rounded-md bg-secondary/60 p-0.5 text-xs">
        <button
          type="button"
          onclick={() => selectStaged(false)}
          aria-pressed={!showStaged}
          class="inline-flex h-7 items-center gap-1.5 rounded-sm px-2.5 transition-colors {!showStaged
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'}"
        >
          Working
          <span class="tabular-nums text-[10px] opacity-70"
            >{workingFiles.length}</span
          >
        </button>
        <button
          type="button"
          onclick={() => selectStaged(true)}
          aria-pressed={showStaged}
          class="inline-flex h-7 items-center gap-1.5 rounded-sm px-2.5 transition-colors {showStaged
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'}"
        >
          Staged
          <span class="tabular-nums text-[10px] opacity-70"
            >{stagedFiles.length}</span
          >
        </button>
      </div>
    </div>
    <div class="flex shrink-0 items-center gap-0.5">
      <button
        type="button"
        onclick={refresh}
        class="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
        aria-label="Refresh changes"
        title="Refresh changes"
        disabled={loading}
      >
        <RefreshCw size={14} class={loading ? "animate-spin" : ""} />
      </button>
    </div>
  </div>

  {#if files.length > 0}
    <div class="flex-1 flex flex-col lg:flex-row overflow-hidden">
      {#if showFileTree}
        <div
          class="max-h-44 w-full shrink-0 overflow-auto border-b border-border lg:max-h-full lg:w-64 lg:border-b-0 lg:border-r"
        >
          <div
            class="sticky top-0 z-10 flex h-8 items-center justify-between border-b border-border/70 bg-background px-3"
          >
            <span class="text-[11px] font-medium text-muted-foreground"
              >Changed files</span
            >
            <span class="text-[10px] tabular-nums text-muted-foreground"
              >{files.length}</span
            >
          </div>
          <div class="py-1">
            {#each files as file (file.path)}
              <button
                type="button"
                class="group flex w-full items-center gap-2 border-l-2 px-2.5 py-1.5 text-left transition-colors {file.path ===
                activeFilePath
                  ? 'border-primary bg-primary/8'
                  : 'border-transparent hover:border-border hover:bg-secondary/30'}"
                onclick={() => loadFileDiff(file.path)}
                aria-current={file.path === activeFilePath ? "true" : undefined}
                title={file.path}
              >
                {#if file.status === "added"}
                  <FilePlus size={14} class="shrink-0 text-success" />
                {:else if file.status === "deleted"}
                  <FileMinus size={14} class="shrink-0 text-error" />
                {:else}
                  <FileEdit size={14} class="shrink-0 text-warning" />
                {/if}
                <span class="min-w-0 flex-1">
                  <span
                    class="block truncate text-xs font-medium text-foreground"
                  >
                    {fileName(file.path)}
                  </span>
                  <span
                    class="mt-0.5 block truncate text-[10px] text-muted-foreground"
                  >
                    {parentPath(file.path)}
                  </span>
                </span>
                <span class="flex w-4 shrink-0 items-center justify-center">
                  {#if loadingFile === file.path}
                    <Loader
                      size={12}
                      class="animate-spin text-muted-foreground"
                    />
                  {/if}
                </span>
              </button>
            {/each}
          </div>
        </div>
      {/if}

      <div class="flex-1 min-w-0 min-h-[280px] overflow-hidden flex flex-col">
        {#if activeFilePath}
          <div
            class="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border/70 px-3"
          >
            <div class="min-w-0">
              <div class="truncate text-xs font-semibold text-foreground">
                {fileName(activeFilePath)}
              </div>
              <div
                class="truncate text-[10px] text-muted-foreground"
                title={activeFilePath}
              >
                {activeFilePath.split("/").slice(0, -1).join("/") ||
                  "Repository root"}
              </div>
            </div>
            <div class="flex shrink-0 items-center gap-2">
              <span class="text-[10px] tabular-nums text-muted-foreground">
                {currentFileIndex + 1} of {files.length}
              </span>
              <div class="flex items-center gap-0.5">
                <button
                  type="button"
                  onclick={() => selectRelativeFile(-1)}
                  disabled={currentFileIndex <= 0}
                  class="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:cursor-not-allowed disabled:opacity-35"
                  aria-label="Previous changed file"
                  title="Previous file · Alt+↑"
                >
                  <ArrowUp size={14} />
                </button>
                <button
                  type="button"
                  onclick={() => selectRelativeFile(1)}
                  disabled={currentFileIndex < 0 ||
                    currentFileIndex >= files.length - 1}
                  class="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:cursor-not-allowed disabled:opacity-35"
                  aria-label="Next changed file"
                  title="Next file · Alt+↓"
                >
                  <ArrowDown size={14} />
                </button>
              </div>
              <div class="inline-flex rounded-md bg-secondary/60 p-0.5 text-xs">
                <button
                  type="button"
                  aria-pressed={viewMode === "unified"}
                  class="h-7 rounded-sm px-2 transition-colors {viewMode ===
                  'unified'
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                  onclick={() => (viewMode = "unified")}
                >
                  Unified
                </button>
                <button
                  type="button"
                  aria-pressed={viewMode === "split"}
                  class="h-7 rounded-sm px-2 transition-colors {viewMode ===
                  'split'
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'}"
                  onclick={() => (viewMode = "split")}
                >
                  Split
                </button>
              </div>
            </div>
          </div>

          <div class="flex-1 overflow-auto">
            {#if loadingFile === activeFilePath}
              <div
                class="flex items-center justify-center h-full text-muted-foreground text-sm"
              >
                Loading diff...
              </div>
            {:else if diffError}
              <div
                class="flex h-full flex-col items-center justify-center gap-3 px-6 text-center"
              >
                <AlertCircle size={20} class="text-error" />
                <div>
                  <div class="text-sm font-medium">Couldn’t load this diff</div>
                  <div class="mt-1 text-xs text-muted-foreground">
                    {diffError}
                  </div>
                </div>
                <button
                  type="button"
                  onclick={() => loadFileDiff(activeFilePath!)}
                  class="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-secondary"
                >
                  Try again
                </button>
              </div>
            {:else if diffFiles.length === 0}
              <div
                class="flex items-center justify-center h-full text-muted-foreground text-sm"
              >
                No diff available
              </div>
            {:else}
              {#each diffFiles as file, i (file.newPath || file.oldPath || i)}
                <div class="mb-4 overflow-hidden">
                  {#if viewMode === "unified"}
                    <div class="font-mono text-xs leading-relaxed">
                      {#each file.hunks as hunk, i (i)}
                        <div
                          class="flex items-center gap-2 px-2 py-0.5 {lineBg(
                            'hunk',
                          )} border-b border-border/50"
                        >
                          <span
                            class="w-10 text-right text-[10px] select-none tabular-nums"
                            >...</span
                          >
                          <span
                            class="w-10 text-right text-[10px] select-none tabular-nums"
                            >...</span
                          >
                          <span class="text-[10px]">{hunk.header}</span>
                        </div>
                        {#each hunk.lines as line, i (i)}
                          <div
                            class="flex items-start gap-2 px-2 py-0.5 {lineBg(
                              line.type,
                            )}"
                          >
                            <span
                              class="w-10 shrink-0 text-right text-[10px] text-muted-foreground select-none tabular-nums {lineNumberBg(
                                line.type,
                              )}"
                            >
                              {line.oldLine ?? ""}
                            </span>
                            <span
                              class="w-10 shrink-0 text-right text-[10px] text-muted-foreground select-none tabular-nums {lineNumberBg(
                                line.type,
                              )}"
                            >
                              {line.newLine ?? ""}
                            </span>
                            <span
                              class="w-3 shrink-0 select-none text-center {line.type ===
                              'add'
                                ? 'text-success'
                                : line.type === 'del'
                                  ? 'text-error'
                                  : 'text-muted-foreground'}"
                            >
                              {line.type === "add"
                                ? "+"
                                : line.type === "del"
                                  ? "-"
                                  : " "}
                            </span>
                            <span class="whitespace-pre {lineText(line.type)}">
                              {line.text}
                            </span>
                          </div>
                        {/each}
                      {/each}
                    </div>
                  {:else}
                    <!-- Split view -->
                    <div class="font-mono text-xs leading-relaxed">
                      {#each file.hunks as hunk, i (i)}
                        <div
                          class="flex items-center gap-2 px-2 py-0.5 {lineBg(
                            'hunk',
                          )} border-b border-border/50"
                        >
                          <span class="text-[10px]">@@ {hunk.header}</span>
                        </div>
                        {#each hunk.lines as line, i (i)}
                          <div
                            class="flex border-b border-border/5 divide-x divide-border/70"
                          >
                            <div
                              class="flex-1 min-w-0 flex items-start gap-2 px-2 py-0.5 {leftLineBg(
                                line.type,
                              )}"
                            >
                              <span
                                class="w-10 shrink-0 text-right text-[10px] text-muted-foreground select-none tabular-nums {lineNumberBg(
                                  line.type,
                                )}"
                              >
                                {line.type !== "add"
                                  ? (line.oldLine ?? "")
                                  : ""}
                              </span>
                              <span
                                class="whitespace-pre {leftLineText(line.type)}"
                              >
                                {line.type === "add"
                                  ? ""
                                  : line.type === "del"
                                    ? "-"
                                    : " "}{line.type === "add" ? "" : line.text}
                              </span>
                            </div>
                            <div
                              class="flex-1 min-w-0 flex items-start gap-2 px-2 py-0.5 {rightLineBg(
                                line.type,
                              )}"
                            >
                              <span
                                class="w-10 shrink-0 text-right text-[10px] text-muted-foreground select-none tabular-nums {lineNumberBg(
                                  line.type,
                                )}"
                              >
                                {line.type !== "del"
                                  ? (line.newLine ?? "")
                                  : ""}
                              </span>
                              <span
                                class="whitespace-pre {rightLineText(
                                  line.type,
                                )}"
                              >
                                {line.type === "del"
                                  ? ""
                                  : line.type === "add"
                                    ? "+"
                                    : " "}{line.type === "del" ? "" : line.text}
                              </span>
                            </div>
                          </div>
                        {/each}
                      {/each}
                    </div>
                  {/if}
                </div>
              {/each}
            {/if}
          </div>
        {:else}
          <div
            class="flex-1 flex items-center justify-center text-muted-foreground text-sm"
          >
            Select a file to view diff
          </div>
        {/if}
      </div>
    </div>
  {:else if loadError}
    <div
      class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center"
    >
      <AlertCircle size={22} class="text-error" />
      <div>
        <div class="text-sm font-medium">Couldn’t load changes</div>
        <div class="mt-1 text-xs text-muted-foreground">{loadError}</div>
      </div>
      <button
        type="button"
        onclick={refresh}
        class="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-secondary"
      >
        Try again
      </button>
    </div>
  {:else if loading}
    <div
      class="flex-1 flex items-center justify-center text-muted-foreground text-sm"
    >
      Loading...
    </div>
  {:else}
    <div
      class="flex flex-1 flex-col items-center justify-center gap-3 text-muted-foreground"
    >
      <Check size={22} class="text-success" />
      <div class="text-center">
        <div class="text-sm font-medium text-foreground">
          {showStaged ? "No staged changes" : "Working tree clean"}
        </div>
        <div class="mt-1 text-xs">
          No {showStaged ? "staged" : "unstaged"} changes were found.
        </div>
      </div>
      <button
        type="button"
        onclick={refresh}
        class="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-secondary hover:text-foreground"
      >
        Refresh
      </button>
    </div>
  {/if}
</div>
