// Shared tauri-store singleton for GUI settings.
//
// Extracted so that both settings.svelte.ts and the theme module share ONE
// Store instance per file — multiple Store instances over the same file would
// clobber each other on save (plugin-store persists the whole collection).
import { Store } from "@tauri-apps/plugin-store";

export const SETTINGS_STORAGE_FILE = "yomi-gui-settings";

let store: Store | null = null;

export async function getSettingsStore(): Promise<Store> {
  if (!store) store = await Store.load(SETTINGS_STORAGE_FILE);
  return store;
}
