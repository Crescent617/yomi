<script lang="ts">
  /**
   * Attachments declared via `<yomi_attachments>` blocks in an assistant
   * message: images render inline as a gallery (click opens the in-app
   * lightbox), text types (Markdown, code, logs…) open in the in-app
   * preview overlay, and everything else renders as chips that open the
   * system default app. Resolution happens on the daemon's host — same
   * safety rules as channel delivery — with bytes fetched over the wire
   * in remote mode, so both modes behave the same. Images that fail to
   * load fall back to chips. Opening a file externally in remote mode
   * downloads it into a local content-keyed cache first and opens that
   * copy.
   */
  import { FileText, ImageOff, Loader2, Paperclip } from "lucide-svelte";
  import { errorMessage, openAttachment, readAttachmentImage } from "../../api";
  import { connectionState } from "../../connection.svelte";
  import { previewFile } from "../../file-preview.svelte";
  import { previewImage } from "../../image-preview.svelte";
  import { getSession, showNotification } from "../../state.svelte";
  import { isTextPreviewable } from "../../utils";

  let {
    paths,
    session_id,
  }: {
    paths: string[];
    session_id: string;
  } = $props();

  const IMAGE_EXTENSIONS = new Set([
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "svg",
    "avif",
    "bmp",
    "ico",
  ]);

  function isImagePath(path: string): boolean {
    const ext = path.split(".").pop()?.toLowerCase() ?? "";
    return IMAGE_EXTENSIONS.has(ext);
  }

  function basename(path: string): string {
    return path.split(/[/\\]/).filter(Boolean).pop() ?? path;
  }

  const baseDir = $derived(getSession(session_id)?.project_path ?? null);
  const uniquePaths = $derived([...new Set(paths)]);
  const isRemote = $derived(connectionState.info?.mode === "remote");

  // Per-image load state: undefined = loading, url = ready, failed = chip.
  // `retryable` marks failures that happened without a workspace path —
  // those retry once baseDir arrives; every other failure is permanent.
  let images = $state<
    Record<string, { url?: string; failed?: boolean; retryable?: boolean }>
  >({});

  const imagePaths = $derived(
    uniquePaths.filter((p) => isImagePath(p) && !images[p]?.failed),
  );
  const filePaths = $derived(
    uniquePaths.filter((p) => !isImagePath(p) || images[p]?.failed === true),
  );

  $effect(() => {
    void baseDir;
    for (const path of uniquePaths.filter(isImagePath)) {
      const state = images[path];
      const needsLoad =
        !state ||
        (state.failed === true && state.retryable === true && !!baseDir);
      if (!needsLoad) continue;
      images[path] = {};
      void loadImage(path);
    }
  });

  async function loadImage(path: string) {
    try {
      const img = await readAttachmentImage(baseDir, path);
      images[path] = { url: `data:${img.mime};base64,${img.data_base64}` };
    } catch {
      // Missing / non-image / oversize: degrade to a plain chip. A failure
      // without a workspace path is not conclusive — retry when it arrives.
      images[path] = { failed: true, retryable: !baseDir };
    }
  }

  // Remote opens download the file first — track the in-flight chip so
  // it can show a spinner and ignore repeat clicks.
  let opening = $state<string | null>(null);

  async function open(path: string) {
    if (opening) return;
    // Text types open in the in-app preview overlay (bytes over the wire,
    // both connection modes); everything else keeps system-default open.
    if (isTextPreviewable(path)) {
      previewFile({
        name: basename(path),
        sub: path,
        markdown: /\.(md|markdown)$/i.test(path),
        source: { kind: "attachment", path, base_dir: baseDir },
      });
      return;
    }
    opening = path;
    try {
      await openAttachment(baseDir, path);
    } catch (e: unknown) {
      showNotification(errorMessage(e), "error");
    } finally {
      opening = null;
    }
  }
</script>

{#if imagePaths.length > 0}
  <div
    class="mt-1.5 grid gap-1.5 {imagePaths.length === 1
      ? 'grid-cols-1'
      : imagePaths.length === 2
        ? 'grid-cols-2'
        : 'grid-cols-2 sm:grid-cols-3'}"
  >
    {#each imagePaths as path (path)}
      {@const img = images[path]}
      {#if img?.url}
        <button
          type="button"
          class="group/img relative cursor-zoom-in overflow-hidden rounded-md border border-border bg-secondary/20 transition-colors hover:border-ring {imagePaths.length ===
          1
            ? 'w-fit max-w-full justify-self-start'
            : 'w-full'}"
          title={path}
          onclick={() => previewImage(img.url!)}
        >
          <img
            src={img.url}
            alt={basename(path)}
            loading="lazy"
            onerror={() => {
              // Corrupt bytes behind an image extension (the backend mime
              // check is extension-based): degrade to a plain chip.
              images[path] = { failed: true };
            }}
            class="object-cover transition-opacity group-hover/img:opacity-90 {imagePaths.length ===
            1
              ? 'max-h-80 max-w-full object-contain'
              : 'aspect-[4/3] w-full'}"
          />
        </button>
      {:else}
        <div
          class="flex items-center justify-center rounded-md border border-border/60 bg-secondary/20 text-muted-foreground {imagePaths.length ===
          1
            ? 'h-24'
            : 'aspect-[4/3]'}"
        >
          <Loader2 size={16} class="animate-spin" />
        </div>
      {/if}
    {/each}
  </div>
{/if}

{#if filePaths.length > 0}
  <div class="mt-1.5 flex flex-wrap gap-1.5">
    {#each filePaths as path (path)}
      {@const failed = images[path]?.failed === true}
      {@const downloading = opening === path}
      <button
        type="button"
        disabled={downloading}
        class="inline-flex max-w-60 items-center gap-1.5 rounded-md border border-border bg-secondary/40 px-2 py-1 text-xs transition-colors {failed
          ? 'text-muted-foreground hover:bg-secondary/60'
          : 'text-muted-foreground hover:bg-secondary hover:text-foreground'} disabled:opacity-70"
        title={failed
          ? `${path} (preview unavailable)`
          : isTextPreviewable(path)
            ? `${path} (click to preview)`
            : isRemote
              ? `${path} (opens a downloaded copy)`
              : path}
        onclick={() => open(path)}
      >
        {#if downloading}
          <Loader2 size={12} class="shrink-0 animate-spin" />
        {:else if failed && isImagePath(path)}
          <ImageOff size={12} class="shrink-0" />
        {:else if isTextPreviewable(path)}
          <FileText size={12} class="shrink-0" />
        {:else}
          <Paperclip size={12} class="shrink-0" />
        {/if}
        <span class="truncate">{basename(path)}</span>
      </button>
    {/each}
  </div>
{/if}
