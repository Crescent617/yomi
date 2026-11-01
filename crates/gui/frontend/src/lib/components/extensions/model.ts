import type {
  ConfiguredResources,
  ExtensionHealth,
  ExtensionInfo,
  RemoveReport,
} from "../../api";

/** Health → presentation. Color never carries meaning alone (DESIGN.md). */
export const HEALTH: Record<
  ExtensionHealth,
  { label: string; dot: string; text: string; hint: string }
> = {
  ok: { label: "OK", dot: "bg-success", text: "text-success", hint: "" },
  modified: {
    label: "Modified locally",
    dot: "bg-warning",
    text: "text-warning",
    hint: "Content differs from install time. Reinstall to restore.",
  },
  foreign: {
    label: "Not installed by yomi",
    dot: "bg-warning",
    text: "text-warning",
    hint: "Hand-placed or broken manifest. Remove will refuse; delete the folder to clean up.",
  },
  unreadable: {
    label: "Unreadable",
    dot: "bg-error",
    text: "text-error",
    hint: "Files can't be read (permission or corruption). Check the folder.",
  },
  oversized: {
    label: "Oversized file",
    dot: "bg-error",
    text: "text-error",
    hint: "A file exceeds the size limit. Move it out or reinstall.",
  },
};

/**
 * Fallback for a health value we don't recognize (a newer daemon). Must NOT
 * look like success — a muted "Unknown" is the honest rendering.
 */
export const UNKNOWN_HEALTH = {
  label: "Unknown state",
  dot: "bg-muted-foreground",
  text: "text-muted-foreground",
  hint: "This yomi can't interpret the package's health. Update yomi.",
};

/** Health lookup that never throws on an unknown variant. */
export function healthMeta(health: ExtensionHealth) {
  return HEALTH[health] ?? UNKNOWN_HEALTH;
}

/** cron adopt outcomes → human labels (install report). */
export const CRON_ADOPT_LABEL: Record<string, string> = {
  created: "created",
  updated: "content updated",
  untouched: "exists, identical",
};

/** hook/bin mount outcomes → human labels (install report). */
export const MOUNT_STATUS_LABEL: Record<string, string> = {
  linked: "linked",
  already: "already linked",
};

/**
 * A source path (vs a GitHub shorthand/URL). Display heuristic only — the
 * daemon decides; use an absolute path or `./` prefix for local sources.
 */
export function isLocal(source: string): boolean {
  return (
    source.startsWith("/") ||
    source.startsWith("./") ||
    /^[A-Za-z]:[\\/]/.test(source) // Windows drive path
  );
}

export function shortRev(rev: string | undefined): string {
  return rev ? rev.slice(0, 8) : "—";
}

/** Compact resource list for the list row, e.g. "2 cron · 1 bin". */
export function resourceSummary(r: ConfiguredResources): string {
  const parts: [number, string, string][] = [
    [r.cron.length, "cron", "cron"],
    [r.hooks.length, "hook", "hooks"],
    [r.bins.length, "bin", "bins"],
    [r.snippets.length, "snippet", "snippets"],
  ];
  const shown = parts
    .filter(([n]) => n > 0)
    .map(([n, one, many]) => `${n} ${n > 1 ? many : one}`);
  return shown.length > 0 ? shown.join(" · ") : "no resources";
}

export function relativeTime(
  iso: string | undefined,
  now: number = Date.now(),
): string {
  if (!iso) return "";
  const diff = now - new Date(iso).getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

/**
 * Warn about a partial removal the GUI would otherwise hide: mounts left
 * behind (slot content changed by hand) or a package folder not deleted
 * (no ownership record). Returns null when the removal was clean.
 */
export function removeNotice(report: RemoveReport): string | null {
  const parts: string[] = [];
  if (report.mounts_left.length > 0) {
    parts.push(
      `Left in place (slot content changed): ${report.mounts_left.join(", ")}`,
    );
  }
  if (!report.ext_dir_removed) {
    parts.push(
      "Package folder kept (no ownership record) — remove it manually.",
    );
  }
  return parts.length > 0 ? parts.join(" ") : null;
}

/**
 * What a remove will actually delete, enumerated for the confirm dialog —
 * the package folder plus every mount and cron job yomi installed for it.
 */
export function removalSummary(ext: ExtensionInfo): string {
  const lines = [`Folder: ${ext.dir}`];
  const r = ext.resources;
  if (r?.cron.length) lines.push(`Cron jobs: ${r.cron.join(", ")}`);
  const mounts = [...(r?.hooks ?? []), ...(r?.bins ?? [])];
  if (mounts.length) lines.push(`Mounts: ${mounts.join(", ")}`);
  return lines.join("\n");
}
