export function elapsedLabel(startedAt: string, now: number): string {
  const elapsedSeconds = Math.max(
    0,
    Math.floor((now - new Date(startedAt).getTime()) / 1000),
  );
  if (elapsedSeconds < 60) return `${elapsedSeconds}s`;
  const minutes = Math.floor(elapsedSeconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

/**
 * Aggregate agent mood for the StatusBar pet indicator. Mirrors the Rust
 * `PetMood` priority ladder (permission > ask > working > idle) using the
 * state the main window already has; the pet window computes the full mood
 * (notices, idle timeout) in `pet.rs`. Snake-case values match `PetMood`
 * so they feed `moodToCodexPetAnimation` directly.
 */
export type AggregateMood = "idle" | "working" | "alert" | "curious";

export function aggregateMood(opts: {
  pendingPermission: boolean;
  pendingAsk: boolean;
  runningCount: number;
}): AggregateMood {
  if (opts.pendingPermission) return "alert";
  if (opts.pendingAsk) return "curious";
  if (opts.runningCount > 0) return "working";
  return "idle";
}

/** Mood chip accent: semantic text class per mood. */
export function moodTextClass(mood: AggregateMood): string {
  switch (mood) {
    case "alert":
      return "text-error";
    case "curious":
      return "text-info";
    case "working":
      return "text-primary";
    case "idle":
      return "text-muted-foreground";
  }
}
