import * as api from "./api";

/**
 * Shared daemon connection state (local vs remote mode).
 *
 * Refreshed once at app startup; connect/disconnect flows reload the
 * window, so the info is always current for the lifetime of the page.
 */
export const connectionState = $state<{ info: api.ConnectionInfo | null }>({
  info: null,
});

export async function refreshConnectionInfo(): Promise<void> {
  try {
    connectionState.info = await api.getConnectionInfo();
  } catch {
    connectionState.info = null;
  }
}
