import { describe, expect, it } from "vitest";
import type { ExtensionHealth, RemoveReport } from "../../api";
import {
  CRON_ADOPT_LABEL,
  HEALTH,
  healthMeta,
  isLocal,
  MOUNT_STATUS_LABEL,
  relativeTime,
  removalSummary,
  removeNotice,
  resourceSummary,
  shortRev,
  UNKNOWN_HEALTH,
} from "./model";

describe("healthMeta", () => {
  it("maps each known health to a labeled style", () => {
    const known: ExtensionHealth[] = [
      "ok",
      "modified",
      "foreign",
      "unreadable",
      "oversized",
    ];
    for (const h of known) {
      expect(healthMeta(h)).toBe(HEALTH[h]);
      expect(healthMeta(h).label).not.toBe("");
    }
  });

  it("falls back to a neutral 'unknown' (never success) for a new variant", () => {
    const meta = healthMeta("futuristic" as ExtensionHealth);
    expect(meta).toBe(UNKNOWN_HEALTH);
    expect(meta.label).not.toBe(HEALTH.ok.label);
    expect(meta.text).not.toBe(HEALTH.ok.text);
  });
});

describe("install report labels", () => {
  it("covers every status the kernel emits", () => {
    expect(CRON_ADOPT_LABEL.created).toBeTruthy();
    expect(CRON_ADOPT_LABEL.updated).toBeTruthy();
    expect(CRON_ADOPT_LABEL.untouched).toBeTruthy();
    expect(MOUNT_STATUS_LABEL.linked).toBeTruthy();
    expect(MOUNT_STATUS_LABEL.already).toBeTruthy();
  });
});

describe("isLocal", () => {
  it("treats absolute and ./-relative paths as local", () => {
    expect(isLocal("/Users/me/ext")).toBe(true);
    expect(isLocal("./my-ext")).toBe(true);
    expect(isLocal("C:\\ext")).toBe(true);
  });

  it("treats GitHub shorthand and URLs as remote", () => {
    expect(isLocal("owner/repo")).toBe(false);
    expect(isLocal("owner/repo/ext/foo@main")).toBe(false);
    expect(isLocal("https://github.com/owner/repo")).toBe(false);
    expect(isLocal("~/my-ext")).toBe(false);
  });
});

describe("shortRev", () => {
  it("truncates a sha and shows an em dash for none", () => {
    expect(shortRev("1a2b3c4d5e6f")).toBe("1a2b3c4d");
    expect(shortRev(undefined)).toBe("—");
  });
});

describe("resourceSummary", () => {
  it("omits zero counts and singularizes one", () => {
    expect(
      resourceSummary({ cron: [], hooks: ["a"], bins: [], snippets: [] }),
    ).toBe("1 hook");
    expect(
      resourceSummary({
        cron: ["a", "b"],
        hooks: [],
        bins: ["x"],
        snippets: ["s"],
      }),
    ).toBe("2 cron · 1 bin · 1 snippet");
  });

  it("says 'no resources' when everything is empty", () => {
    expect(
      resourceSummary({ cron: [], hooks: [], bins: [], snippets: [] }),
    ).toBe("no resources");
  });
});

describe("relativeTime", () => {
  const now = Date.parse("2026-11-01T12:00:00Z");
  it("renders coarse relative buckets", () => {
    expect(relativeTime(undefined, now)).toBe("");
    expect(relativeTime("2026-11-01T11:59:40Z", now)).toBe("just now");
    expect(relativeTime("2026-11-01T11:30:00Z", now)).toBe("30m ago");
    expect(relativeTime("2026-11-01T06:00:00Z", now)).toBe("6h ago");
    expect(relativeTime("2026-10-29T12:00:00Z", now)).toBe("3d ago");
  });
});

describe("removalSummary", () => {
  it("enumerates the folder, cron jobs, and mounts", () => {
    const s = removalSummary({
      name: "demo",
      version: "1.0.0",
      description: "",
      dir: "/data/extensions/demo",
      health: "ok",
      source: "owner/repo",
      resources: {
        cron: ["ext:demo:tick"],
        hooks: ["pre_tool_use/50-guard"],
        bins: ["recall"],
        snippets: [],
      },
    });
    expect(s).toContain("/data/extensions/demo");
    expect(s).toContain("ext:demo:tick");
    expect(s).toContain("pre_tool_use/50-guard");
    expect(s).toContain("recall");
  });

  it("handles a package with no recorded resources", () => {
    const s = removalSummary({
      name: "x",
      version: "1.0.0",
      description: "",
      dir: "/data/extensions/x",
      health: "foreign",
    });
    expect(s).toContain("/data/extensions/x");
    expect(s).not.toContain("Cron jobs");
  });
});

describe("removeNotice", () => {
  const base: RemoveReport = {
    name: "demo",
    cron_removed: [],
    mounts_left: [],
    mounts_removed: [],
    ext_dir_removed: true,
  };

  it("returns null for a clean removal", () => {
    expect(removeNotice(base)).toBeNull();
  });

  it("reports mounts left in place", () => {
    const notice = removeNotice({
      ...base,
      mounts_left: ["bin/recall"],
    });
    expect(notice).toContain("bin/recall");
  });

  it("reports a kept package folder", () => {
    const notice = removeNotice({ ...base, ext_dir_removed: false });
    expect(notice).toContain("manually");
  });

  it("combines both problems", () => {
    const notice = removeNotice({
      ...base,
      mounts_left: ["bin/x"],
      ext_dir_removed: false,
    });
    expect(notice).toContain("bin/x");
    expect(notice).toContain("manually");
  });
});
