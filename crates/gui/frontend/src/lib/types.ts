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
