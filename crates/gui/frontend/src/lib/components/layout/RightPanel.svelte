<script lang="ts">
  import { onMount } from "svelte";
  import {
    FileDiff as FileDiffIcon,
    GitCommit,
    GitBranch,
    RefreshCw,
    FilePlus,
    FileMinus,
    FileEdit,
    ChevronRight,
    ChevronDown,
    Folder,
    FolderOpen,
    Loader,
    PanelLeftClose,
    PanelLeftOpen,
    PanelRightClose,
  } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import * as api from "../../api";

  let { session, onClose }: { session: SessionState | null; onClose?: () => void } = $props();

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
    oldStart: number;
    oldLines: number;
    newStart: number;
    newLines: number;
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
  let showStaged = $state(false);
  let loading = $state(false);
  let loadingFile = $state<string | null>(null);
  let activeFilePath = $state<string | null>(null);
  let viewMode = $state<"unified" | "split">("unified");
  let diffFiles = $state<DiffFile[]>([]);
  let loadVersion = 0;
  let diffLoadVersion = 0;
  let lastPath = "";
  let lastStaged = false;
  let lastRawDiff = "";
  let expandedDirs = $state<Set<string>>(new Set());
  let showFileTree = $state(true);

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
          const oldStart = parseInt(match[1], 10);
          const oldLines = match[2] ? parseInt(match[2], 10) : 1;
          const newStart = parseInt(match[3], 10);
          const newLines = match[4] ? parseInt(match[4], 10) : 1;
          currentHunk = {
            oldStart,
            oldLines,
            newStart,
            newLines,
            header: match[5] || "",
            lines: [],
          };
          oldLine = oldStart;
          newLine = newStart;
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
            currentHunk.lines.push({ type: "add", oldLine: null, newLine, text });
            newLine++;
          } else if (ch === "-") {
            currentHunk.lines.push({ type: "del", oldLine, newLine: null, text });
            oldLine++;
          } else if (ch === "\\") {
            currentHunk.lines.push({ type: "context", oldLine, newLine, text: line });
          } else {
            currentHunk.lines.push({ type: "context", oldLine, newLine, text: line });
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

  interface TreeNode {
    name: string;
    path: string;
    isDir: boolean;
    children: TreeNode[];
    fileIndex?: number;
    status?: string;
  }

  function buildTree(files: FileSummary[]): TreeNode[] {
    const root: TreeNode[] = [];
    for (let i = 0; i < files.length; i++) {
      const f = files[i];
      const parts = f.path.split("/");
      let current = root;
      let currentPath = "";
      for (let j = 0; j < parts.length; j++) {
        const part = parts[j];
        currentPath = currentPath ? `${currentPath}/${part}` : part;
        const isLast = j === parts.length - 1;
        let node = current.find((n) => n.name === part);
        if (!node) {
          node = {
            name: part,
            path: currentPath,
            isDir: !isLast,
            children: [],
            fileIndex: isLast ? i : undefined,
            status: isLast ? f.status : undefined,
          };
          current.push(node);
        }
        if (!isLast) {
          current = node.children;
        }
      }
    }
    function sortNodes(nodes: TreeNode[]) {
      nodes.sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return a.name.localeCompare(b.name);
      });
      for (const n of nodes) {
        if (n.isDir) sortNodes(n.children);
      }
    }
    sortNodes(root);
    return root;
  }

  function compressTree(nodes: TreeNode[]): TreeNode[] {
    const compressed = nodes.map((node) => {
      if (node.isDir) {
        const compressedChildren = compressTree(node.children);
        return { ...node, children: compressedChildren };
      }
      return node;
    });
    const result: TreeNode[] = [];
    for (const node of compressed) {
      if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
        const child = node.children[0];
        result.push({
          ...node,
          name: `${node.name}/${child.name}`,
          path: child.path,
          children: child.children,
        });
      } else {
        result.push(node);
      }
    }
    return result;
  }

  const treeNodes = $derived(compressTree(buildTree(files)));

  function collectAllDirPaths(nodes: TreeNode[]): Set<string> {
    const paths = new Set<string>();
    for (const n of nodes) {
      if (n.isDir) {
        paths.add(n.path);
        for (const p of collectAllDirPaths(n.children)) {
          paths.add(p);
        }
      }
    }
    return paths;
  }

  $effect(() => {
    if (treeNodes.length > 0) {
      expandedDirs = collectAllDirPaths(treeNodes);
    }
  });

  function toggleDir(path: string) {
    const next = new Set(expandedDirs);
    if (next.has(path)) {
      next.delete(path);
    } else {
      next.add(path);
    }
    expandedDirs = next;
  }

  async function loadFileList() {
    const path = session?.projectPath;
    if (!path) return;
    const staged = showStaged;
    const currentVersion = ++loadVersion;

    loading = true;
    try {
      const result = await api.getGitDiffSummary(path, staged);
      if (currentVersion !== loadVersion) return;
      files = result ?? [];
      activeFilePath = null;
      diffFiles = [];
      lastRawDiff = "";
      expandedDirs = new Set();
      if (files.length > 0) {
        loadFileDiff(files[0].path);
      }
    } catch (e) {
      console.error("Failed to load git diff summary:", e);
      files = [];
    } finally {
      if (currentVersion === loadVersion) {
        loading = false;
      }
    }
  }

  async function loadFileDiff(filePath: string) {
    const path = session?.projectPath;
    if (!path) return;

    const idx = files.findIndex((f) => f.path === filePath);
    if (idx < 0) return;

    activeFilePath = filePath;

    const currentStaged = showStaged;
    const currentDiffVersion = ++diffLoadVersion;
    loadingFile = filePath;

    try {
      const raw = await api.getGitFileDiffRaw(path, filePath, currentStaged);
      if (currentDiffVersion !== diffLoadVersion || showStaged !== currentStaged) return;
      if (files.findIndex((f) => f.path === filePath) < 0 || activeFilePath !== filePath) return;

      if (!raw) {
        diffFiles = [];
        lastRawDiff = "";
        return;
      }

      lastRawDiff = raw;
      diffFiles = parseDiff(raw);
    } catch (e) {
      console.error("Failed to load file diff:", e);
      diffFiles = [];
      lastRawDiff = "";
    } finally {
      if (loadingFile === filePath) loadingFile = null;
    }
  }

  function maybeLoad() {
    const path = session?.projectPath;
    if (!path) {
      files = [];
      diffFiles = [];
      lastRawDiff = "";
      return;
    }
    if (path === lastPath && showStaged === lastStaged) return;
    lastPath = path;
    lastStaged = showStaged;
    loadFileList();
  }

  onMount(() => {
    maybeLoad();
  });

  $effect(() => {
    const path = session?.projectPath;
    if (path && path !== lastPath) {
      maybeLoad();
    }
  });



  function lineBg(type: DiffLine["type"]) {
    switch (type) {
      case "add":
        return "bg-green-500/10 dark:bg-green-500/15";
      case "del":
        return "bg-red-500/10 dark:bg-red-500/15";
      case "hunk":
        return "bg-muted text-muted-foreground";
      default:
        return "";
    }
  }

  function lineText(type: DiffLine["type"]) {
    switch (type) {
      case "add":
        return "text-green-700 dark:text-green-400";
      case "del":
        return "text-red-700 dark:text-red-400";
      case "hunk":
        return "text-muted-foreground";
      default:
        return "text-foreground";
    }
  }

  function leftLineBg(type: DiffLine["type"]) {
    // left column: old file — del lines are red, add lines are blank placeholder
    if (type === "del") return "bg-red-500/10 dark:bg-red-500/15";
    if (type === "add") return "bg-muted/30";
    return "";
  }

  function leftLineText(type: DiffLine["type"]) {
    if (type === "del") return "text-red-700 dark:text-red-400";
    if (type === "add") return "text-muted-foreground";
    return "text-foreground";
  }

  function rightLineBg(type: DiffLine["type"]) {
    // right column: new file — add lines are green, del lines are blank placeholder
    if (type === "add") return "bg-green-500/10 dark:bg-green-500/15";
    if (type === "del") return "bg-muted/30";
    return "";
  }

  function rightLineText(type: DiffLine["type"]) {
    if (type === "add") return "text-green-700 dark:text-green-400";
    if (type === "del") return "text-muted-foreground";
    return "text-foreground";
  }
</script>

<div class="flex flex-col h-full bg-background min-w-0">
  <!-- Header -->
  <div class="shrink-0 px-3 py-2 border-b border-border flex items-center justify-between">
    <div class="flex items-center gap-2">
      <FileDiffIcon size={14} class="text-muted-foreground" />
      <span class="text-sm font-medium">Git Diff</span>
    </div>
    <div class="flex items-center gap-1">
      <button
        type="button"
        onclick={() => showFileTree = !showFileTree}
        class="p-1 rounded-md text-muted-foreground hover:bg-secondary transition-colors"
        title={showFileTree ? "Hide file tree" : "Show file tree"}
      >
        {#if showFileTree}
          <PanelLeftClose size={14} />
        {:else}
          <PanelLeftOpen size={14} />
        {/if}
      </button>
      <button
        type="button"
        onclick={maybeLoad}
        class="p-1 rounded-md text-muted-foreground hover:bg-secondary transition-colors"
        title="Refresh"
      >
        <RefreshCw size={14} />
      </button>
      <button
        type="button"
        onclick={() => { showStaged = false; maybeLoad(); }}
        class="px-2 py-0.5 text-xs rounded transition-colors {showStaged ? 'text-muted-foreground hover:bg-secondary' : 'bg-primary/10 text-primary'}"
      >
        <GitBranch size={12} class="inline mr-1" /> Working
      </button>
      <button
        type="button"
        onclick={() => { showStaged = true; maybeLoad(); }}
        class="px-2 py-0.5 text-xs rounded transition-colors {showStaged ? 'bg-primary/10 text-primary' : 'text-muted-foreground hover:bg-secondary'}"
      >
        <GitCommit size={12} class="inline mr-1" /> Staged
      </button>
      {#if onClose}
        <button
          type="button"
          onclick={onClose}
          class="p-1 rounded-md text-muted-foreground hover:bg-secondary transition-colors"
          title="Close"
        >
          <PanelRightClose size={14} />
        </button>
      {/if}
    </div>
  </div>

  {#if files.length > 0}
    <div class="flex-1 flex flex-col lg:flex-row overflow-hidden">
      {#if showFileTree}
        <div class="shrink-0 lg:w-[220px] w-full max-h-[200px] lg:max-h-full overflow-auto border-b lg:border-b-0 lg:border-r border-border">
          <div class="px-2 py-1 text-xs font-medium text-muted-foreground border-b border-border sticky top-0 bg-background z-10">
            {files.length} file{files.length === 1 ? "" : "s"}
          </div>
          {#snippet renderTree(nodes: TreeNode[], depth: number)}
            {#each nodes as node (node.path)}
              {#if node.isDir}
                <button
                  class="w-full text-left flex items-center gap-1 transition-colors hover:bg-secondary text-foreground"
                  style="padding-left: {depth * 12 + 8}px; padding-top: 4px; padding-bottom: 4px;"
                  onclick={() => toggleDir(node.path)}
                >
                  {#if expandedDirs.has(node.path)}
                    <ChevronDown size={12} class="text-muted-foreground shrink-0" />
                    <FolderOpen size={12} class="text-muted-foreground shrink-0" />
                  {:else}
                    <ChevronRight size={12} class="text-muted-foreground shrink-0" />
                    <Folder size={12} class="text-muted-foreground shrink-0" />
                  {/if}
                  <span class="text-xs truncate">{node.name}</span>
                </button>
                {#if expandedDirs.has(node.path)}
                  {@render renderTree(node.children, depth + 1)}
                {/if}
              {:else}
                <button
                  class="w-full text-left flex items-center gap-1 transition-colors {node.path === activeFilePath
                    ? 'bg-primary/10 text-primary'
                    : 'text-foreground hover:bg-secondary'}"
                  style="padding-left: {depth * 12 + 8}px; padding-top: 4px; padding-bottom: 4px;"
                  onclick={() => loadFileDiff(node.path)}
                >
                  {#if node.status === "added"}
                    <FilePlus size={12} class="text-emerald-600 shrink-0" />
                  {:else if node.status === "deleted"}
                    <FileMinus size={12} class="text-red-600 shrink-0" />
                  {:else}
                    <FileEdit size={12} class="text-amber-600 shrink-0" />
                  {/if}
                  <span class="text-xs truncate flex-1">{node.name}</span>
                  <span class="w-4 shrink-0 flex items-center justify-center">
                    {#if loadingFile === node.path}
                      <Loader size={12} class="text-muted-foreground animate-spin" />
                    {/if}
                  </span>
                </button>
              {/if}
            {/each}
          {/snippet}
          {@render renderTree(treeNodes, 0)}
        </div>
      {/if}

      <div class="flex-1 min-w-0 min-h-[280px] overflow-hidden flex flex-col">
        {#if activeFilePath}
          <div class="shrink-0 flex items-center justify-between px-2 py-1 border-b border-border">
            <span class="text-xs text-muted-foreground truncate">{activeFilePath}</span>
            <div class="flex gap-1">
              <button
                class="text-xs px-2 py-0.5 rounded {viewMode === 'unified' ? 'bg-secondary' : ''}"
                onclick={() => (viewMode = "unified")}
              >
                Unified
              </button>
              <button
                class="text-xs px-2 py-0.5 rounded {viewMode === 'split' ? 'bg-secondary' : ''}"
                onclick={() => (viewMode = "split")}
              >
                Split
              </button>
            </div>
          </div>

          <div class="flex-1 overflow-auto">
            {#if loadingFile === activeFilePath}
              <div class="flex items-center justify-center h-full text-muted-foreground text-sm">
                Loading diff...
              </div>
            {:else if diffFiles.length === 0}
              <div class="flex items-center justify-center h-full text-muted-foreground text-sm">
                No diff available
              </div>
            {:else}
              {#each diffFiles as file}
                <div class="mb-4 overflow-hidden">
                  {#if viewMode === "unified"}
                    <div class="font-mono text-xs leading-relaxed">
                      {#each file.hunks as hunk}
                        <div class="flex items-center gap-2 px-2 py-0.5 {lineBg('hunk')} border-b border-border/50">
                          <span class="w-10 text-right text-[10px] select-none tabular-nums">...</span>
                          <span class="w-10 text-right text-[10px] select-none tabular-nums">...</span>
                          <span class="text-[10px]">{hunk.header}</span>
                        </div>
                        {#each hunk.lines as line}
                          <div class="flex items-start gap-2 px-2 py-0.5 {lineBg(line.type)}">
                            <span class="w-10 text-right text-[10px] text-muted-foreground select-none tabular-nums shrink-0">
                              {line.oldLine ?? ""}
                            </span>
                            <span class="w-10 text-right text-[10px] text-muted-foreground select-none tabular-nums shrink-0">
                              {line.newLine ?? ""}
                            </span>
                            <span class="whitespace-pre-wrap {lineText(line.type)}">
                              {line.type === "add" ? "+" : line.type === "del" ? "-" : " "}{line.text}
                            </span>
                          </div>
                        {/each}
                      {/each}
                    </div>
                  {:else}
                    <!-- Split view -->
                    <div class="font-mono text-xs leading-relaxed">
                      {#each file.hunks as hunk}
                        <div class="flex items-center gap-2 px-2 py-0.5 {lineBg('hunk')} border-b border-border/50">
                          <span class="text-[10px]">@@ {hunk.header}</span>
                        </div>
                        {#each hunk.lines as line}
                          <div class="flex border-b border-border/5">
                            <div class="flex-1 min-w-0 flex items-start gap-2 px-2 py-0.5 {leftLineBg(line.type)}">
                              <span class="w-10 text-right text-[10px] text-muted-foreground select-none tabular-nums shrink-0">
                                {line.type !== 'add' ? (line.oldLine ?? '') : ''}
                              </span>
                              <span class="whitespace-pre {leftLineText(line.type)}">
                                {line.type === 'add' ? '' : line.type === 'del' ? '-' : ' '}{line.type === 'add' ? '' : line.text}
                              </span>
                            </div>
                            <div class="flex-1 min-w-0 flex items-start gap-2 px-2 py-0.5 {rightLineBg(line.type)}">
                              <span class="w-10 text-right text-[10px] text-muted-foreground select-none tabular-nums shrink-0">
                                {line.type !== 'del' ? (line.newLine ?? '') : ''}
                              </span>
                              <span class="whitespace-pre {rightLineText(line.type)}">
                                {line.type === 'del' ? '' : line.type === 'add' ? '+' : ' '}{line.type === 'del' ? '' : line.text}
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
          <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
            Select a file to view diff
          </div>
        {/if}
      </div>
    </div>
  {:else if loading}
    <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
      Loading...
    </div>
  {:else}
    <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
      No {showStaged ? "staged" : "unstaged"} changes.
    </div>
  {/if}
</div>
