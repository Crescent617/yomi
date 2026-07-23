<script lang="ts">
  import { onMount } from "svelte";
  import {
    AlertCircle,
    ChevronLeft,
    ChevronRight,
    FileCode,
    PanelLeftOpen,
    RefreshCw,
    RotateCcw,
    Save,
    Zap,
  } from "lucide-svelte";
  import * as api from "../../api";
  import { appState, showNotification } from "../../state.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";

  let {
    onToggleLeftPanel,
  }: {
    onToggleLeftPanel?: () => void;
  } = $props();

  type SaveDiagnostic = {
    message: string;
    line: number | null;
    column: number | null;
  };

  type TomlToken = {
    text: string;
    className: string;
  };

  const lineHeight = 24;

  let content = $state("");
  let disk_content = $state("");
  let filePath = $state("");
  let loading = $state(true);
  let reloading = $state(false);
  let saving = $state(false);
  let saveError = $state<SaveDiagnostic | null>(null);
  let full_config = $state("");

  let restarting = $state(false);
  let restartConfirmOpen = $state(false);
  let effectiveCollapsed = $state(false);

  let textareaRef = $state<HTMLTextAreaElement>();
  let scrollTop = $state(0);
  let scrollLeft = $state(0);
  let currentLine = $state(1);

  const dirty = $derived(content !== disk_content);
  const lines = $derived(content.split("\n"));
  const highlightedLines = $derived(highlightToml(content));
  const errorLine = $derived(saveError?.line ?? null);

  const restartMessage =
    "Restart the daemon to apply config changes?\n\nAll running sessions and tasks will be interrupted. Chat history is preserved.";

  const daemonButtonTitle = $derived(
    dirty
      ? "Save config changes before restarting"
      : "Restart the daemon to apply config changes",
  );

  const saveStatus = $derived.by(() => {
    if (saving) return "Saving…";
    if (saveError) return "Save failed";
    if (dirty) return "Unsaved changes";
    if (appState.config_restart_required) {
      return "Saved to daemon · Restart required";
    }
    if (appState.config_applied) return "Applied";
    return "Saved to daemon";
  });

  const saveStatusClass = $derived(
    saveError
      ? "text-error"
      : dirty || appState.config_restart_required
        ? "text-warning"
        : appState.config_applied
          ? "text-success"
          : "text-muted-foreground",
  );

  $effect(() => {
    appState.config_dirty = dirty;
  });

  function findUnquoted(text: string, target: string): number {
    let quote = "";
    let escaped = false;
    for (let i = 0; i < text.length; i += 1) {
      const char = text[i];
      if (escaped) {
        escaped = false;
        continue;
      }
      if (quote === '"' && char === "\\") {
        escaped = true;
        continue;
      }
      if (char === '"' || char === "'") {
        if (!quote) quote = char;
        else if (quote === char) quote = "";
        continue;
      }
      if (!quote && char === target) return i;
    }
    return -1;
  }

  function valueTokens(value: string): TomlToken[] {
    const result: TomlToken[] = [];
    const pattern =
      /("(?:\\.|[^"\\])*"|'[^']*'|\b(?:true|false)\b|[-+]?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)/g;
    let offset = 0;
    for (const match of value.matchAll(pattern)) {
      const index = match.index ?? 0;
      if (index > offset) {
        result.push({ text: value.slice(offset, index), className: "" });
      }
      const token = match[0];
      result.push({
        text: token,
        className:
          token.startsWith('"') || token.startsWith("'")
            ? "text-success"
            : token === "true" || token === "false"
              ? "text-warning"
              : "text-info",
      });
      offset = index + token.length;
    }
    if (offset < value.length) {
      result.push({ text: value.slice(offset), className: "" });
    }
    return result;
  }

  function highlightTomlLine(line: string): TomlToken[] {
    const commentIndex = findUnquoted(line, "#");
    const body = commentIndex >= 0 ? line.slice(0, commentIndex) : line;
    const comment = commentIndex >= 0 ? line.slice(commentIndex) : "";
    const result: TomlToken[] = [];

    if (body.trimStart().startsWith("[")) {
      result.push({ text: body, className: "text-info" });
    } else {
      const equalsIndex = findUnquoted(body, "=");
      if (equalsIndex >= 0) {
        const key = body.slice(0, equalsIndex);
        const leading = key.match(/^\s*/)?.[0] ?? "";
        if (leading) result.push({ text: leading, className: "" });
        result.push({
          text: key.slice(leading.length),
          className: "text-primary",
        });
        result.push({ text: "=", className: "text-muted-foreground" });
        result.push(...valueTokens(body.slice(equalsIndex + 1)));
      } else {
        result.push({ text: body, className: "" });
      }
    }

    if (comment) {
      result.push({ text: comment, className: "text-muted-foreground" });
    }
    return result;
  }

  function highlightToml(source: string): TomlToken[][] {
    let multiline = "";
    return source.split("\n").map((line) => {
      if (multiline) {
        const end = line.indexOf(multiline);
        if (end < 0) return [{ text: line, className: "text-success" }];
        const tokens = [
          {
            text: line.slice(0, end + multiline.length),
            className: "text-success",
          },
        ];
        const rest = line.slice(end + multiline.length);
        multiline = "";
        return [...tokens, ...highlightTomlLine(rest)];
      }

      const basic = line.indexOf('"""');
      const literal = line.indexOf("'''");
      const start =
        basic < 0 ? literal : literal < 0 ? basic : Math.min(basic, literal);
      if (start < 0) return highlightTomlLine(line);

      const delimiter = start === basic ? '"""' : "'''";
      if (line.indexOf(delimiter, start + delimiter.length) >= 0) {
        return highlightTomlLine(line);
      }
      multiline = delimiter;
      return [
        ...highlightTomlLine(line.slice(0, start)),
        { text: line.slice(start), className: "text-success" },
      ];
    });
  }

  function parseSaveDiagnostic(error: unknown): SaveDiagnostic {
    const message = api.errorMessage(error);
    const location = message.match(/line\s+(\d+)\s*,\s*column\s+(\d+)/i);
    return {
      message,
      line: location ? Number(location[1]) : null,
      column: location ? Number(location[2]) : null,
    };
  }

  function syncScroll(element: HTMLTextAreaElement = textareaRef!) {
    if (!element) return;
    scrollTop = element.scrollTop;
    scrollLeft = element.scrollLeft;
  }

  function updateCursor(element: HTMLTextAreaElement = textareaRef!) {
    if (!element) return;
    currentLine = element.value
      .slice(0, element.selectionStart)
      .split("\n").length;
  }

  function jumpToLine(line: number, column = 1) {
    if (!textareaRef) return;
    const sourceLines = content.split("\n");
    const safeLine = Math.max(1, Math.min(line, sourceLines.length));
    const safeColumn = Math.max(
      1,
      Math.min(column, sourceLines[safeLine - 1].length + 1),
    );
    let offset = 0;
    for (let i = 0; i < safeLine - 1; i += 1) {
      offset += sourceLines[i].length + 1;
    }
    offset += safeColumn - 1;

    textareaRef.focus();
    textareaRef.setSelectionRange(offset, offset);
    textareaRef.scrollTop = Math.max(
      0,
      (safeLine - 1) * lineHeight - textareaRef.clientHeight / 2,
    );
    syncScroll(textareaRef);
    currentLine = safeLine;
  }

  function handleBeforeUnload(event: BeforeUnloadEvent) {
    if (!dirty) return;
    event.preventDefault();
    event.returnValue = "";
  }

  async function doRestartDaemon() {
    restartConfirmOpen = false;
    if (restarting || dirty) return;
    restarting = true;
    try {
      await api.restartDaemon();
      appState.config_restart_required = false;
      appState.config_applied = true;
      showNotification("Configuration applied", "success");
      const toml = await api.getConfigToml().catch(() => null);
      if (toml) full_config = toml.full_config;
    } catch (e: unknown) {
      showNotification(
        `Failed to restart daemon: ${api.errorMessage(e)}`,
        "error",
      );
    } finally {
      restarting = false;
    }
  }

  async function loadFromDisk(initial = false): Promise<boolean> {
    if (initial) loading = true;
    else reloading = true;
    const previousDiskContent = disk_content;
    try {
      const toml = await api.getConfigToml();
      content = toml.content;
      disk_content = toml.content;
      filePath = toml.path;
      full_config = toml.full_config;
      saveError = null;
      if (!initial && toml.content !== previousDiskContent) {
        appState.config_restart_required = true;
        appState.config_applied = false;
      }
      currentLine = 1;
      scrollTop = 0;
      scrollLeft = 0;
      return true;
    } catch (e: unknown) {
      console.error("Failed to load config:", e);
      showNotification(
        `Failed to ${initial ? "load config" : "reload from daemon"}: ${api.errorMessage(e)}`,
        "error",
      );
      return false;
    } finally {
      if (initial) loading = false;
      else reloading = false;
    }
  }

  async function reload() {
    if (loading || reloading || saving) return;
    if (
      dirty &&
      !window.confirm(
        "Reload from daemon and discard your unsaved config changes?",
      )
    ) {
      return;
    }
    if (await loadFromDisk()) {
      showNotification("Reloaded from daemon", "success");
    }
  }

  async function save() {
    if (!dirty || saving || loading || reloading) return;
    const snapshot = content;
    saving = true;
    saveError = null;
    try {
      await api.saveConfigToml(snapshot);
      disk_content = snapshot;
      appState.config_restart_required = true;
      appState.config_applied = false;
      showNotification("Config saved to daemon", "success");
    } catch (e: unknown) {
      console.error("Failed to save config:", e);
      saveError = parseSaveDiagnostic(e);
      showNotification("Save failed", "error");
    } finally {
      saving = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "s" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void save();
    }
  }

  onMount(() => {
    void loadFromDisk(true);
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
      appState.config_dirty = false;
    };
  });
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  <div
    class="shrink-0 flex flex-wrap items-center justify-between gap-2 px-4 py-2 border-b border-border bg-card"
  >
    <div class="flex items-center gap-2 min-w-0">
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={() => onToggleLeftPanel()}
          class="lg:hidden p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground mr-1"
          title="Toggle sidebar"
        >
          <PanelLeftOpen size={16} />
        </button>
      {/if}
      <FileCode class="w-4 h-4 text-muted-foreground shrink-0" />
      <span class="text-sm font-medium shrink-0">Kernel</span>
      <span class="text-xs text-muted-foreground truncate max-w-[300px]"
        >{filePath}</span
      >
    </div>

    <div class="flex flex-wrap items-center justify-end gap-2">
      {#if saveStatus}
        <span class="text-xs {saveStatusClass}">{saveStatus}</span>
      {/if}
      <button
        type="button"
        onclick={reload}
        disabled={loading || reloading || saving}
        class="inline-flex items-center gap-1 rounded-md border border-border bg-secondary/40 px-2.5 py-1 text-xs text-foreground transition-colors hover:bg-secondary/80 disabled:opacity-50"
      >
        <RotateCcw class="w-3 h-3 {reloading ? 'animate-spin' : ''}" />
        Reload
      </button>
      <button
        type="button"
        onclick={save}
        disabled={!dirty || saving || loading || reloading}
        class="inline-flex items-center gap-1 rounded-md border border-primary/30 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary transition-colors hover:border-primary/40 hover:bg-primary/15 disabled:opacity-50"
      >
        <Save class="w-3 h-3" />
        Save
      </button>
      <div class="h-5 w-px bg-border"></div>
      <span title={daemonButtonTitle}>
        <button
          type="button"
          onclick={() => (restartConfirmOpen = true)}
          disabled={restarting || dirty}
          class="inline-flex items-center gap-1 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-1 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/15 disabled:pointer-events-none disabled:opacity-50"
        >
          <RefreshCw class="w-3 h-3 {restarting ? 'animate-spin' : ''}" />
          {restarting ? "Restarting…" : "Restart"}
        </button>
      </span>
    </div>
  </div>

  <div class="flex-1 flex flex-col md:flex-row min-h-0 overflow-hidden">
    <section
      class="order-1 flex-1 md:w-3/5 min-w-0 min-h-[18rem] flex flex-col"
      aria-label="Config editor"
    >
      {#if loading}
        <div class="flex items-center justify-center h-full">
          <div
            class="w-6 h-6 border-2 border-primary border-t-transparent rounded-full animate-spin"
          ></div>
        </div>
      {:else}
        <div
          class="relative flex-1 min-h-0 overflow-hidden bg-background focus-within:shadow-[inset_0_0_0_1px_var(--color-ring)]"
        >
          {#if currentLine}
            <div
              class="absolute left-12 right-0 h-6 pointer-events-none {currentLine ===
              errorLine
                ? 'bg-error/10'
                : 'bg-primary/10'}"
              style:top={`${16 + (currentLine - 1) * lineHeight - scrollTop}px`}
            ></div>
          {/if}
          {#if errorLine && errorLine !== currentLine}
            <div
              class="absolute left-12 right-0 h-6 bg-error/10 pointer-events-none"
              style:top={`${16 + (errorLine - 1) * lineHeight - scrollTop}px`}
            ></div>
          {/if}

          <div
            class="absolute inset-y-0 left-12 right-0 overflow-hidden pointer-events-none font-mono text-sm leading-6"
            aria-hidden="true"
          >
            <pre
              class="min-w-full w-max p-4 text-foreground"
              style:transform={`translate(${-scrollLeft}px, ${-scrollTop}px)`}>{#each highlightedLines as tokens, index (index)}<span
                  class="block h-6 min-w-full"
                  >{#each tokens as token, tokenIndex (`${tokenIndex}:${token.text}`)}<span
                      class={token.className}>{token.text}</span
                    >{/each}</span
                >{/each}</pre>
          </div>

          <textarea
            bind:this={textareaRef}
            bind:value={content}
            wrap="off"
            oninput={(event) => {
              saveError = null;
              updateCursor(event.currentTarget);
            }}
            onkeyup={(event) => updateCursor(event.currentTarget)}
            onclick={(event) => updateCursor(event.currentTarget)}
            onselect={(event) => updateCursor(event.currentTarget)}
            onscroll={(event) => syncScroll(event.currentTarget)}
            onkeydown={handleKeydown}
            class="absolute inset-y-0 left-12 right-0 w-[calc(100%-3rem)] h-full resize-none border-0 bg-transparent p-4 font-mono text-sm leading-6 text-transparent caret-foreground selection:bg-primary/20 focus-visible:outline-none"
            spellcheck={false}
            aria-label="Config TOML"
          ></textarea>

          <div
            class="absolute inset-y-0 left-0 z-10 w-12 overflow-hidden border-r border-border bg-muted/30 pt-4 font-mono text-xs leading-6 text-muted-foreground"
            aria-hidden="true"
          >
            <div style:transform={`translateY(${-scrollTop}px)`}>
              {#each lines as _, index (index)}
                <button
                  type="button"
                  tabindex="-1"
                  onclick={() => jumpToLine(index + 1)}
                  class="block h-6 w-full pr-2 text-right hover:text-foreground {index +
                    1 ===
                  errorLine
                    ? 'bg-error/10 text-error'
                    : index + 1 === currentLine
                      ? 'bg-primary/10 text-primary'
                      : ''}"
                >
                  {index + 1}
                </button>
              {/each}
            </div>
          </div>
        </div>

        {#if saveError}
          <button
            type="button"
            onfocus={() => {
              if (saveError?.line) {
                jumpToLine(saveError.line, saveError.column ?? 1);
              }
            }}
            onclick={() => {
              if (saveError?.line) {
                jumpToLine(saveError.line, saveError.column ?? 1);
              } else {
                textareaRef?.focus();
              }
            }}
            class="flex w-full shrink-0 items-start gap-2 border-t border-error/30 bg-error/10 px-3 py-2 text-left text-xs text-error"
          >
            <AlertCircle class="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              {#if saveError.line}
                <span class="font-medium"
                  >Line {saveError.line}, column {saveError.column ?? 1} ·
                </span>
              {/if}
              <span class="whitespace-pre-wrap">{saveError.message}</span>
            </span>
          </button>
        {/if}
      {/if}
    </section>

    <section
      class="order-2 shrink-0 min-w-0 border-t md:border-t-0 md:border-l border-border flex flex-col {effectiveCollapsed
        ? 'md:w-10'
        : 'md:w-2/5 h-1/3 md:h-auto'}"
      aria-label="Effective config"
    >
      <div
        class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border bg-card"
      >
        {#if !effectiveCollapsed}
          <Zap class="w-4 h-4 text-muted-foreground shrink-0" />
          <span class="text-sm font-medium truncate">Effective Config</span>
        {/if}
        <button
          type="button"
          onclick={() => (effectiveCollapsed = !effectiveCollapsed)}
          class="ml-auto rounded p-1 text-muted-foreground hover:bg-secondary hover:text-foreground"
          title={effectiveCollapsed
            ? "Expand Effective Config"
            : "Collapse Effective Config"}
        >
          {#if effectiveCollapsed}
            <ChevronLeft class="w-4 h-4" />
          {:else}
            <ChevronRight class="w-4 h-4" />
          {/if}
        </button>
      </div>
      {#if !effectiveCollapsed}
        <div class="flex-1 min-h-0 overflow-auto p-3">
          {#if loading}
            <div class="flex items-center justify-center py-8">
              <div
                class="w-4 h-4 border-2 border-primary border-t-transparent rounded-full animate-spin"
              ></div>
            </div>
          {:else if full_config}
            <pre
              class="min-w-max whitespace-pre text-xs font-mono text-muted-foreground leading-relaxed">{full_config}</pre>
          {:else}
            <div class="text-sm text-muted-foreground">
              Failed to load effective config
            </div>
          {/if}
        </div>
      {/if}
    </section>
  </div>

  <ConfirmDialog
    open={restartConfirmOpen}
    title="Restart"
    message={restartMessage}
    confirmText="Restart"
    onConfirm={doRestartDaemon}
    onCancel={() => (restartConfirmOpen = false)}
  />
</div>
