export type SessionTimeGroupLabel =
  | "30 minutes ago"
  | "1 hour ago"
  | "3 hours ago"
  | "12 hours ago"
  | "Today"
  | "Yesterday"
  | "A week ago"
  | "A month ago"
  | "Older";

export interface SessionTimeGroup<T> {
  label: SessionTimeGroupLabel;
  sessions: T[];
}

const GROUP_LABELS: SessionTimeGroupLabel[] = [
  "30 minutes ago",
  "1 hour ago",
  "3 hours ago",
  "12 hours ago",
  "Today",
  "Yesterday",
  "A week ago",
  "A month ago",
  "Older",
];

function localDayOffset(now: Date, days: number): number {
  const date = new Date(now);
  date.setHours(0, 0, 0, 0);
  date.setDate(date.getDate() - days);
  return date.getTime();
}

export function groupSessionsByTime<T extends { updated_at?: string }>(
  sessions: T[],
  nowMs: number = Date.now(),
): SessionTimeGroup<T>[] {
  const now = new Date(nowMs);
  const today = localDayOffset(now, 0);
  const thirtyMinutesAgo = nowMs - 30 * 60 * 1000;
  const oneHourAgo = nowMs - 60 * 60 * 1000;
  const threeHoursAgo = nowMs - 3 * 60 * 60 * 1000;
  const twelveHoursAgo = nowMs - 12 * 60 * 60 * 1000;
  const yesterday = localDayOffset(now, 1);
  const sevenDaysAgo = localDayOffset(now, 7);
  const thirtyDaysAgo = localDayOffset(now, 30);
  const groups = new Map<SessionTimeGroupLabel, T[]>(
    GROUP_LABELS.map((label) => [label, []]),
  );

  for (const session of sessions) {
    const timestamp = session.updated_at
      ? new Date(session.updated_at).getTime()
      : Number.NaN;
    let label: SessionTimeGroupLabel = "Older";

    if (Number.isFinite(timestamp)) {
      if (timestamp >= thirtyMinutesAgo) label = "30 minutes ago";
      else if (timestamp >= oneHourAgo) label = "1 hour ago";
      else if (timestamp >= threeHoursAgo) label = "3 hours ago";
      else if (timestamp >= twelveHoursAgo) label = "12 hours ago";
      else if (timestamp >= today) label = "Today";
      else if (timestamp >= yesterday) label = "Yesterday";
      else if (timestamp >= sevenDaysAgo) label = "A week ago";
      else if (timestamp >= thirtyDaysAgo) label = "A month ago";
    }

    groups.get(label)?.push(session);
  }

  return GROUP_LABELS.flatMap((label) => {
    const groupedSessions = groups.get(label) ?? [];
    return groupedSessions.length > 0
      ? [{ label, sessions: groupedSessions }]
      : [];
  });
}
