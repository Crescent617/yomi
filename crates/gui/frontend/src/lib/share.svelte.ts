/**
 * Global share-dialog state: any surface can request a share card for an
 * answer; a single ShareCardDialog mounted in Layout renders it.
 */

export interface ShareRequest {
  /** Markdown source of the answer */
  content: string;
  sessionTitle?: string;
  date: Date;
}

export const shareState = $state<{ request: ShareRequest | null }>({
  request: null,
});

export function requestShare(request: ShareRequest): void {
  shareState.request = request;
}

export function closeShare(): void {
  shareState.request = null;
}
