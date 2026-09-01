// Shared text-file preview state: previewable attachment chips open an
// in-app overlay (Markdown rendered, other text syntax-highlighted) via
// previewFile(); the overlay component lives at the app root next to the
// image lightbox. Inline sources (no file behind them, e.g. session
// rules) render the same way minus the external-open action.
export interface FilePreviewTarget {
  /** Display name in the overlay header. */
  name: string;
  /** Quiet mono subline (file path, scope hint); omitted when null. */
  sub?: string | null;
  /** Markdown renders with full typography; other text is highlighted. */
  markdown: boolean;
  source:
    | { kind: "attachment"; path: string; base_dir: string | null }
    | { kind: "inline"; text: string };
}

export const filePreview = $state<{ target: FilePreviewTarget | null }>({
  target: null,
});

export function previewFile(target: FilePreviewTarget): void {
  filePreview.target = target;
}

export function closeFilePreview(): void {
  filePreview.target = null;
}
