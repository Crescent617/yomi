// Shared types used by api.ts and state.svelte.ts to avoid circular imports

export interface TaggedContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  signature?: string;
  image_url?: { url: string; detail?: string };
  url?: string;
  mime_type?: string;
}

export interface ContentBlockText {
  type: "text";
  text: string;
}

export interface ContentBlockImage {
  type: "image_url";
  image_url: { url: string };
}

export type ContentBlock =
  | ContentBlockText
  | ContentBlockImage
  | TaggedContentBlock;

export function buildContentBlocks(
  text: string,
  images: Array<{ url: string }>,
): TaggedContentBlock[] {
  const blocks: TaggedContentBlock[] = [];
  for (const img of images) {
    blocks.push({
      type: "image_url",
      image_url: { url: img.url, detail: "auto" },
    });
  }
  const trimmed = text.trim();
  if (trimmed) {
    blocks.push({ type: "text", text: trimmed });
  }
  if (blocks.length === 0) {
    blocks.push({ type: "text", text: "" });
  }
  return blocks;
}
