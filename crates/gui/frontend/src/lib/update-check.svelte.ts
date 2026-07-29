import { getVersion } from "@tauri-apps/api/app";

/**
 * Polls GitHub Releases for a newer yomi version. Checking is silent by
 * design: network failures, rate limits, and malformed payloads only flip
 * `status` to "error" — the UI never nags about the check itself, it just
 * stays on the current version.
 */

export type UpdateCheckStatus =
  | "idle"
  | "checking"
  | "available"
  | "up_to_date"
  | "error";

export interface UpdateCheckState {
  status: UpdateCheckStatus;
  /** Current app version, plain semver (e.g. "0.7.23"). */
  current_version: string | null;
  /** Latest release version, normalized without the "v" prefix. */
  latest_version: string | null;
  /** Release page URL on GitHub. */
  release_url: string | null;
  /** ISO publish timestamp of the latest release. */
  published_at: string | null;
  /** Epoch ms of the last completed check. */
  checked_at: number | null;
}

export const updateCheckState = $state<UpdateCheckState>({
  status: "idle",
  current_version: null,
  latest_version: null,
  release_url: null,
  published_at: null,
  checked_at: null,
});

const LATEST_RELEASE_URL =
  "https://api.github.com/repos/Crescent617/yomi/releases/latest";
const INITIAL_DELAY_MS = 5_000;
const CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000;
const REQUEST_TIMEOUT_MS = 10_000;

export interface ParsedVersion {
  major: number;
  minor: number;
  patch: number;
  prerelease: string[] | null;
}

export function parseVersion(raw: string): ParsedVersion | null {
  const match =
    /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
      raw.trim(),
    );
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ? match[4].split(".") : null,
  };
}

/**
 * Semver ordering: returns 1 when a > b, -1 when a < b, 0 when equal, and
 * null when either side is unparseable.
 */
export function compareVersions(a: string, b: string): number | null {
  const va = parseVersion(a);
  const vb = parseVersion(b);
  if (!va || !vb) return null;
  for (const key of ["major", "minor", "patch"] as const) {
    if (va[key] !== vb[key]) return va[key] > vb[key] ? 1 : -1;
  }
  if (!va.prerelease && !vb.prerelease) return 0;
  if (!va.prerelease) return 1;
  if (!vb.prerelease) return -1;
  const len = Math.max(va.prerelease.length, vb.prerelease.length);
  for (let i = 0; i < len; i++) {
    const x = va.prerelease[i];
    const y = vb.prerelease[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    if (x === y) continue;
    const xNumeric = /^\d+$/.test(x);
    const yNumeric = /^\d+$/.test(y);
    if (xNumeric && yNumeric) return Number(x) > Number(y) ? 1 : -1;
    // Numeric identifiers rank below alphanumeric ones (semver §11).
    if (xNumeric) return -1;
    if (yNumeric) return 1;
    return x < y ? -1 : 1;
  }
  return 0;
}

export interface LatestRelease {
  version: string;
  url: string;
  published_at: string | null;
}

/** Validate the GitHub API payload; returns null for anything unexpected. */
export function releaseFromApi(json: unknown): LatestRelease | null {
  if (!json || typeof json !== "object") return null;
  const { tag_name, html_url, published_at } = json as Record<string, unknown>;
  if (typeof tag_name !== "string" || typeof html_url !== "string") {
    return null;
  }
  const version = tag_name.trim().replace(/^v/, "");
  if (!parseVersion(version)) return null;
  return {
    version,
    url: html_url,
    published_at: typeof published_at === "string" ? published_at : null,
  };
}

async function fetchLatestRelease(): Promise<LatestRelease | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch(LATEST_RELEASE_URL, {
      headers: { Accept: "application/vnd.github+json" },
      signal: controller.signal,
    });
    if (!response.ok) return null;
    return releaseFromApi(await response.json());
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

export async function checkForUpdates(): Promise<void> {
  if (updateCheckState.status === "checking") return;
  updateCheckState.status = "checking";
  try {
    const [current, release] = await Promise.all([
      getVersion(),
      fetchLatestRelease(),
    ]);
    updateCheckState.current_version = current;
    updateCheckState.checked_at = Date.now();
    if (!release) {
      updateCheckState.status = "error";
      return;
    }
    const comparison = compareVersions(release.version, current);
    if (comparison === null) {
      updateCheckState.status = "error";
      return;
    }
    updateCheckState.latest_version = release.version;
    updateCheckState.release_url = release.url;
    updateCheckState.published_at = release.published_at;
    updateCheckState.status = comparison > 0 ? "available" : "up_to_date";
  } catch {
    updateCheckState.status = "error";
  }
}

let initialTimer: ReturnType<typeof setTimeout> | null = null;
let intervalTimer: ReturnType<typeof setInterval> | null = null;
let started = false;

/** Delay the first check so it does not compete with startup work. */
export function startUpdateChecker(): void {
  if (started) return;
  started = true;
  initialTimer = setTimeout(() => {
    initialTimer = null;
    void checkForUpdates();
    intervalTimer = setInterval(() => {
      void checkForUpdates();
    }, CHECK_INTERVAL_MS);
  }, INITIAL_DELAY_MS);
}

export function stopUpdateChecker(): void {
  started = false;
  if (initialTimer) {
    clearTimeout(initialTimer);
    initialTimer = null;
  }
  if (intervalTimer) {
    clearInterval(intervalTimer);
    intervalTimer = null;
  }
}
