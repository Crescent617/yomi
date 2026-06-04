// Shared types used by api.ts and state.svelte.ts to avoid circular imports

export interface TaggedContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  signature?: string;
  imageUrl?: { url: string };
  url?: string;
  mimeType?: string;
}

export interface ContentBlockText {
  type: "text";
  text: string;
}

export interface ContentBlockImage {
  type: "imageUrl";
  imageUrl: { url: string };
}

export type ContentBlock = ContentBlockText | ContentBlockImage | TaggedContentBlock;
