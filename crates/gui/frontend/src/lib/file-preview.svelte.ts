// Shared text-file preview state: previewable attachment chips open an
// in-app overlay (Markdown rendered, other text syntax-highlighted) via
// previewFile(); the overlay component lives at the app root next to the
// image lightbox.
export interface FilePreviewTarget {
  path: string;
  base_dir: string | null;
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
