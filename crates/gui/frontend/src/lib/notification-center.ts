export interface SessionCompletionNotification {
  id: string;
  sessionId: string;
  title: string;
  projectId: string | null;
  completedAt: string;
  read: boolean;
}

export const MAX_SESSION_NOTIFICATIONS = 100;

export function seedRunningSessionStatuses(
  statuses: Map<string, string>,
  sessionIds: string[],
): void {
  for (const sessionId of sessionIds) {
    if (!statuses.has(sessionId)) statuses.set(sessionId, "streaming");
  }
}

export function didSessionComplete(
  previousStatus: string | undefined,
  nextStatus: string,
): boolean {
  return (
    previousStatus != null && previousStatus !== "idle" && nextStatus === "idle"
  );
}

export function addSessionCompletion(
  notifications: SessionCompletionNotification[],
  notification: SessionCompletionNotification,
): SessionCompletionNotification[] {
  return [
    notification,
    ...notifications.filter(
      (item) => item.sessionId !== notification.sessionId,
    ),
  ].slice(0, MAX_SESSION_NOTIFICATIONS);
}

export function relativeNotificationTime(iso: string, now: number): string {
  const elapsed = Math.max(0, now - new Date(iso).getTime());
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}
