import * as api from "./api";
import type { FavoriteAnswer } from "./api";
import { showNotification } from "./state.svelte";

/**
 * Global favorites state.
 *
 * The full list doubles as the lookup table for star state on assistant
 * messages (keyed by `session_id:message_id`), so it is loaded once and
 * kept in sync optimistically on every mutation.
 */

const MAX_FAVORITES = 500;

export const favoritesState = $state({
  items: [] as FavoriteAnswer[],
  loaded: false,
});

export function favoriteKey(sessionId: string, messageId: string): string {
  return `${sessionId}:${messageId}`;
}

const favoriteIdByMessage = $derived.by(() => {
  const map = new Map<string, string>();
  for (const item of favoritesState.items) {
    map.set(favoriteKey(item.session_id, item.message_id), item.id);
  }
  return map;
});

export function favoriteIdFor(
  sessionId: string,
  messageId: string,
): string | undefined {
  return favoriteIdByMessage.get(favoriteKey(sessionId, messageId));
}

export async function loadFavorites(force = false): Promise<void> {
  if (favoritesState.loaded && !force) return;
  try {
    favoritesState.items = await api.listFavorites(undefined, MAX_FAVORITES, 0);
    favoritesState.loaded = true;
  } catch (e) {
    console.error("Failed to load favorites:", e);
  }
}

export interface ToggleFavoriteInput {
  session_id: string;
  message_id: string;
  content: string;
  session_title?: string;
  message_created_at?: string;
}

/** Message keys with an in-flight toggle, guarding against double-clicks. */
const pendingToggles = new Set<string>();

/** Toggle the favorite for a message. Returns the new favorited state. */
export async function toggleFavorite(
  input: ToggleFavoriteInput,
): Promise<boolean> {
  const key = favoriteKey(input.session_id, input.message_id);
  if (pendingToggles.has(key)) {
    return favoriteIdFor(input.session_id, input.message_id) !== undefined;
  }
  pendingToggles.add(key);
  try {
    const existingId = favoriteIdFor(input.session_id, input.message_id);
    if (existingId) {
      await deleteFavorite(existingId);
      return false;
    }
    try {
      const added = await api.addFavorite(
        input.session_id,
        input.message_id,
        input.content,
        input.session_title,
        input.message_created_at,
      );
      favoritesState.items = [
        added,
        ...favoritesState.items.filter((item) => item.id !== added.id),
      ];
      return true;
    } catch (e) {
      console.error("Failed to add favorite:", e);
      showNotification("Failed to add favorite", "error");
      return false;
    }
  } finally {
    pendingToggles.delete(key);
  }
}

export async function deleteFavorite(id: string): Promise<void> {
  const prev = favoritesState.items;
  favoritesState.items = prev.filter((item) => item.id !== id);
  try {
    await api.removeFavorite(id);
  } catch (e) {
    favoritesState.items = prev;
    console.error("Failed to remove favorite:", e);
    showNotification("Failed to remove favorite", "error");
  }
}

export async function saveFavoriteNote(
  id: string,
  note?: string,
): Promise<void> {
  const prev = favoritesState.items;
  favoritesState.items = prev.map((item) =>
    item.id === id ? { ...item, note: note || undefined } : item,
  );
  try {
    await api.updateFavoriteNote(id, note || undefined);
  } catch (e) {
    favoritesState.items = prev;
    console.error("Failed to update favorite note:", e);
    showNotification("Failed to update note", "error");
  }
}
