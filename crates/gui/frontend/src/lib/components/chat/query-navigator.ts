import { textFromBlocks } from "../../session";
import type { Message } from "../../state.svelte";
import { userTextForHeight } from "./user-text";

export interface UserQueryMarker {
  id: string;
  label: string;
}

const MAX_LABEL_LENGTH = 72;

export function summarizeUserQuery(text: string): string {
  const normalized = userTextForHeight(text).replace(/\s+/g, " ").trim();
  const characters = Array.from(normalized);
  if (characters.length <= MAX_LABEL_LENGTH) return normalized;
  return `${characters.slice(0, MAX_LABEL_LENGTH - 1).join("")}…`;
}

export function userQueryMarkers(messages: Message[]): UserQueryMarker[] {
  return messages
    .filter((message) => message.type === "user")
    .map((message) => {
      const label = summarizeUserQuery(textFromBlocks(message.content));
      const hasImage = message.content.some(
        (block) => block.type === "image_url" && block.image_url?.url,
      );
      return {
        id: message.id,
        label: label || (hasImage ? "Image attachment" : "User query"),
      };
    });
}
