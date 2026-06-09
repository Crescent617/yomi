import { fsProvider } from "./fs/factory";
import { homeDir } from "@tauri-apps/api/path";
import type { FileEntry } from "./fs/provider";

export function createFilePicker() {
  let showFilePicker = $state(false);
  let filePickerAnchor = $state(0);
  let fileEntries = $state<FileEntry[]>([]);
  let selectedFileIdx = $state(0);
  let filePickerRoot = $state("");
  let filePickerQuery = $state("");
  let filePickerDir = $state("");
  let homeDirPath = $state("");
  const dirCache = $state<Map<string, FileEntry[]>>(new Map());
  let lastRequestId = $state(0);

  async function ensureHomeDir() {
    if (!homeDirPath) {
      try {
        homeDirPath = await homeDir();
      } catch {
        homeDirPath = "";
      }
    }
    return homeDirPath;
  }

  async function updateFilePicker(root: string) {
    const home = await ensureHomeDir();
    if (!root) root = home;
    filePickerRoot = root;
    const currentRequestId = ++lastRequestId;

    const { dir, filter } = resolvePickerDir(filePickerQuery, root, home);
    filePickerDir = dir;

    if (!dir) {
      fileEntries = [];
      return;
    }

    try {
      const cached = dirCache.get(dir);
      let entries: FileEntry[];
      if (cached) {
        entries = cached;
      } else {
        const list = await fsProvider.listDir(dir);
        if (currentRequestId !== lastRequestId) return; // stale
        entries = list.sort((a, b) => {
          if (a.isDirectory && !b.isDirectory) return -1;
          if (!a.isDirectory && b.isDirectory) return 1;
          return a.name.localeCompare(b.name);
        });
        dirCache.set(dir, entries);
      }

      if (currentRequestId !== lastRequestId) return; // stale

      if (filter) {
        const lowerFilter = filter.toLowerCase();
        fileEntries = entries.filter((e) =>
          e.name.toLowerCase().includes(lowerFilter)
        );
      } else {
        fileEntries = entries;
      }
      selectedFileIdx = 0;
    } catch (e) {
      if (currentRequestId !== lastRequestId) return; // stale
      console.error("Failed to load dir:", filePickerDir, e);
      fileEntries = [];
    }
  }

  function openPicker(anchor: number, query: string, root: string) {
    if (showFilePicker && query === filePickerQuery && anchor === filePickerAnchor && root === filePickerRoot) {
      return;
    }
    filePickerAnchor = anchor;
    filePickerQuery = query;
    showFilePicker = true;
    updateFilePicker(root);
  }

  function closePicker() {
    showFilePicker = false;
  }

  function clearCache() {
    dirCache.clear();
  }

  function buildPickerPath(entry: FileEntry, isDir: boolean): string {
    const suffix = isDir ? "/" : "";
    const selected = entry.name + suffix;

    if (filePickerQuery.endsWith("/") || filePickerQuery === "") {
      return filePickerQuery + selected;
    } else if (filePickerQuery.includes("/")) {
      const lastSlash = filePickerQuery.lastIndexOf("/");
      return filePickerQuery.slice(0, lastSlash + 1) + selected;
    } else {
      if (filePickerQuery === "~") {
        return "~" + "/" + selected;
      }
      return selected;
    }
  }

  function enterDir(entry: FileEntry): string {
    const newQuery = buildPickerPath(entry, true);
    filePickerQuery = newQuery;
    updateFilePicker(filePickerRoot);
    return newQuery;
  }

  function acceptFile(entry: FileEntry): string {
    return buildPickerPath(entry, false);
  }

  function handleKeydown(e: KeyboardEvent): boolean {
    if (!showFilePicker) return false;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedFileIdx = Math.min(selectedFileIdx + 1, Math.max(0, fileEntries.length - 1));
      return true;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedFileIdx = Math.max(selectedFileIdx - 1, 0);
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      const entry = fileEntries[selectedFileIdx];
      if (entry) {
        if (entry.isDirectory) {
          enterDir(entry);
        } else {
          return true; // signal caller to accept file
        }
      }
      return true;
    }
    if (e.key === "Escape") {
      closePicker();
      return true;
    }
    return false;
  }

  return {
    get show() { return showFilePicker; },
    get anchor() { return filePickerAnchor; },
    get query() { return filePickerQuery; },
    get dir() { return filePickerDir; },
    get root() { return filePickerRoot; },
    get entries() { return fileEntries; },
    get selectedIdx() { return selectedFileIdx; },
    get homeDirPath() { return homeDirPath; },
    open: openPicker,
    close: closePicker,
    update: updateFilePicker,
    clearCache,
    enterDir,
    acceptFile,
    buildPickerPath,
    handleKeydown,
  };
}

export function resolvePickerDir(
  query: string,
  root: string,
  home: string
): { dir: string; filter: string } {
  let dir: string;
  let filter: string;

  if (!query) {
    dir = root;
    filter = "";
  } else if (query.startsWith("/")) {
    if (query.endsWith("/")) {
      dir = query;
      filter = "";
    } else {
      const lastSlash = query.lastIndexOf("/");
      if (lastSlash <= 0) {
        dir = "/";
        filter = query.slice(1);
      } else {
        dir = query.slice(0, lastSlash + 1);
        filter = query.slice(lastSlash + 1);
      }
    }
  } else if (query.startsWith("~")) {
    const rest = query.slice(1);
    const homeBase = home.endsWith("/") ? home : home + "/";
    const subPath = rest.startsWith("/") ? rest.slice(1) : rest;
    if (rest === "" || rest === "/") {
      dir = home;
      filter = "";
    } else if (subPath.endsWith("/")) {
      dir = homeBase + subPath;
      filter = "";
    } else {
      const lastSlash = subPath.lastIndexOf("/");
      if (lastSlash === -1) {
        dir = home;
        filter = subPath;
      } else {
        dir = homeBase + subPath.slice(0, lastSlash + 1);
        filter = subPath.slice(lastSlash + 1);
      }
    }
  } else {
    if (query.endsWith("/")) {
      dir = root ? `${root}/${query}` : query;
      filter = "";
    } else {
      const lastSlash = query.lastIndexOf("/");
      if (lastSlash === -1) {
        dir = root;
        filter = query;
      } else {
        dir = root ? `${root}/${query.slice(0, lastSlash + 1)}` : query.slice(0, lastSlash + 1);
        filter = query.slice(lastSlash + 1);
      }
    }
  }

  if (dir !== "/" && dir.endsWith("/")) {
    dir = dir.slice(0, -1);
  }

  return { dir, filter };
}
