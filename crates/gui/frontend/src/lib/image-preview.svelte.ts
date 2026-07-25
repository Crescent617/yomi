// Shared image-preview state: any image thumbnail can open a full-size
// lightbox via previewImage(); the overlay component lives at the app root.
export const imagePreview = $state<{ src: string | null }>({ src: null });

export function previewImage(src: string): void {
  imagePreview.src = src;
}

export function closeImagePreview(): void {
  imagePreview.src = null;
}
