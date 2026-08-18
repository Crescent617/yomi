<script lang="ts">
  import { onMount } from "svelte";
  import {
    RefreshCw,
    FilePlus,
    FileMinus,
    FileEdit,
    Files,
    ArrowLeft,
    ChevronLeft,
    ChevronRight,
    AlertCircle,
    Check,
  } from "lucide-svelte";
  import type { ThemedToken } from "shiki";
  import type { SessionState } from "../../state.svelte";
  import * as api from "../../api";
  import DiffCodeLine from "./DiffCodeLine.svelte";
  import LoadingPlaceholder from "../ui/LoadingPlaceholder.svelte";
  import {
    highlightDiffHunks,
    resolveDiffLanguagePath,
  } from "./diff-highlight";

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
    oldTokens?: ThemedToken[];
    newTokens?: ThemedToken[];
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
  let filePickerOpen = $state(false);
  let fileTabsElement = $state<HTMLDivElement>();

  function scrollActiveFileTab() {
    requestAnimationFrame(() => {
      fileTabsElement
        ?.querySelector<HTMLElement>("[aria-current='page']")
        ?.scrollIntoView({
          behavior: "smooth",
          block: "nearest",
          inline: "nearest",
        });
    });
  }

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
    diffFiles = [];
    scrollActiveFileTab();
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
      const parsedDiff = parseDiff(raw);
      await Promise.all(
        parsedDiff.map((file) =>
          highlightDiffHunks(
            file.hunks,
            resolveDiffLanguagePath(file.oldPath, file.newPath, filePath),
          ),
        ),
      );
      if (
        currentDiffVersion === diffLoadVersion &&
        showStaged === currentStaged &&
        activeFilePath === filePath
      ) {
        diffFiles = parsedDiff;
      }
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
        <RefreshCw size={14} />
      </button>
    </div>
  </div>

  {#if loading && files.length === 0}
    <LoadingPlaceholder label="Loading changes" class="flex-1" />
  {:else if files.length > 0}
    <div class="flex h-10 shrink-0 border-b border-border/70 bg-card/40">
      <div
        class="scrollbar-hidden flex min-w-0 flex-1 overflow-x-auto"
        bind:this={fileTabsElement}
        role="tablist"
        aria-label="Changed files"
        onwheel={(e) => {
          // 竖向滚轮转横向滚动：overflow-x 容器对鼠标竖轮天然无响应。
          if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
            e.preventDefault();
            e.currentTarget.scrollLeft += e.deltaY;
          }
        }}
      >
        {#each files as file (file.path)}
          <button
            type="button"
            role="tab"
            aria-selected={file.path === activeFilePath}
            aria-current={file.path === activeFilePath ? "page" : undefined}
            class="relative inline-flex h-full max-w-52 shrink-0 items-center gap-1.5 border-r border-border/60 px-3 text-xs transition-colors {file.path ===
            activeFilePath
              ? 'bg-background text-foreground after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:bg-primary'
              : 'text-muted-foreground hover:bg-secondary/40 hover:text-foreground'}"
            onclick={() => loadFileDiff(file.path)}
            title={file.path}
          >
            {#if file.status === "added"}
              <FilePlus size={13} class="shrink-0 text-success" />
            {:else if file.status === "deleted"}
              <FileMinus size={13} class="shrink-0 text-error" />
            {:else}
              <FileEdit size={13} class="shrink-0 text-warning" />
            {/if}
            <span class="truncate">{fileName(file.path)}</span>
          </button>
        {/each}
      </div>

      <div
        class="relative z-10 flex shrink-0 items-center gap-1 border-l border-border bg-background px-1.5"
      >
        <button
          type="button"
          onclick={() => selectRelativeFile(-1)}
          disabled={currentFileIndex <= 0}
          class="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:cursor-not-allowed disabled:opacity-35"
          aria-label="Previous changed file"
          title="Previous file · Alt+↑"
        >
          <ChevronLeft size={14} />
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
          <ChevronRight size={14} />
        </button>
        <div class="relative">
          <button
            type="button"
            onclick={() => (filePickerOpen = !filePickerOpen)}
            class="inline-flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            aria-label="All changed files"
            aria-expanded={filePickerOpen}
            title="All changed files"
          >
            <Files size={14} />
            <span class="tabular-nums">{files.length}</span>
          </button>
          {#if filePickerOpen}
            <button
              type="button"
              aria-label="Close changed files"
              class="fixed inset-0 z-10"
              onclick={() => (filePickerOpen = false)}
            ></button>
            <div
              class="absolute right-0 top-full z-20 mt-1 max-h-[min(24rem,60vh)] w-72 overflow-auto rounded-md border border-border bg-popover py-1 shadow-lg"
            >
              {#each files as file (file.path)}
                <button
                  type="button"
                  class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left transition-colors {file.path ===
                  activeFilePath
                    ? 'bg-primary/8'
                    : 'hover:bg-secondary/50'}"
                  onclick={() => {
                    filePickerOpen = false;
                    void loadFileDiff(file.path);
                  }}
                  aria-current={file.path === activeFilePath
                    ? "page"
                    : undefined}
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
                </button>
              {/each}
            </div>
          {/if}
        </div>
        <div class="inline-flex rounded-md bg-secondary/60 p-0.5 text-[11px]">
          <button
            type="button"
            aria-pressed={viewMode === "unified"}
            class="h-7 rounded-sm px-2 transition-colors {viewMode === 'unified'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (viewMode = "unified")}
          >
            Unified
          </button>
          <button
            type="button"
            aria-pressed={viewMode === "split"}
            class="h-7 rounded-sm px-2 transition-colors {viewMode === 'split'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (viewMode = "split")}
          >
            Split
          </button>
        </div>
      </div>
    </div>

    <div class="flex-1 overflow-hidden flex flex-col">
      <div class="flex-1 min-w-0 min-h-[280px] overflow-hidden flex flex-col">
        {#if activeFilePath}
          <div class="flex-1 overflow-auto">
            {#if loadingFile === activeFilePath}
              <LoadingPlaceholder label="Loading diff" />
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
                    <div class="font-mono text-xs leading-5">
                      {#each file.hunks as hunk, i (i)}
                        <div
                          class="flex items-center {lineBg(
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
                          <div class="flex items-start {lineBg(line.type)}">
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
                            <DiffCodeLine
                              tokens={line.type === "del"
                                ? line.oldTokens
                                : line.newTokens}
                              text={line.text}
                            />
                          </div>
                        {/each}
                      {/each}
                    </div>
                  {:else}
                    <!-- Split view -->
                    <div class="font-mono text-xs leading-5">
                      {#each file.hunks as hunk, i (i)}
                        <div
                          class="flex items-center {lineBg(
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
                              class="flex-1 min-w-0 flex items-start {leftLineBg(
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
                              <span class={leftLineText(line.type)}>
                                {line.type === "add"
                                  ? ""
                                  : line.type === "del"
                                    ? "-"
                                    : " "}<DiffCodeLine
                                  tokens={line.oldTokens}
                                  text={line.type === "add" ? "" : line.text}
                                />
                              </span>
                            </div>
                            <div
                              class="flex-1 min-w-0 flex items-start {rightLineBg(
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
                              <span class={rightLineText(line.type)}>
                                {line.type === "del"
                                  ? ""
                                  : line.type === "add"
                                    ? "+"
                                    : " "}<DiffCodeLine
                                  tokens={line.newTokens}
                                  text={line.type === "del" ? "" : line.text}
                                />
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
