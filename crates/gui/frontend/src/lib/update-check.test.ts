import { describe, expect, test } from "vitest";
import {
  compareVersions,
  parseVersion,
  releaseFromApi,
} from "./update-check.svelte";

describe("parseVersion", () => {
  test("parses plain semver", () => {
    expect(parseVersion("0.7.23")).toEqual({
      major: 0,
      minor: 7,
      patch: 23,
      prerelease: null,
    });
  });

  test("accepts a leading v", () => {
    expect(parseVersion("v1.2.3")).toEqual({
      major: 1,
      minor: 2,
      patch: 3,
      prerelease: null,
    });
  });

  test("parses prerelease and ignores build metadata", () => {
    expect(parseVersion("1.0.0-rc.1+build.5")).toEqual({
      major: 1,
      minor: 0,
      patch: 0,
      prerelease: ["rc", "1"],
    });
  });

  test("rejects malformed versions", () => {
    expect(parseVersion("1.2")).toBeNull();
    expect(parseVersion("")).toBeNull();
    expect(parseVersion("latest")).toBeNull();
    expect(parseVersion("1.2.3.4")).toBeNull();
  });
});

describe("compareVersions", () => {
  test("orders by major, minor, patch", () => {
    expect(compareVersions("1.0.0", "0.9.9")).toBe(1);
    expect(compareVersions("0.8.0", "0.7.23")).toBe(1);
    expect(compareVersions("0.7.24", "0.7.23")).toBe(1);
    expect(compareVersions("0.7.23", "0.7.23")).toBe(0);
    expect(compareVersions("0.7.22", "0.7.23")).toBe(-1);
  });

  test("ignores the v prefix", () => {
    expect(compareVersions("v0.7.24", "0.7.23")).toBe(1);
    expect(compareVersions("v0.7.23", "v0.7.23")).toBe(0);
  });

  test("release ranks above prerelease of the same version", () => {
    expect(compareVersions("1.0.0", "1.0.0-rc.1")).toBe(1);
    expect(compareVersions("1.0.0-rc.1", "1.0.0")).toBe(-1);
    expect(compareVersions("1.0.0-rc.2", "1.0.0-rc.1")).toBe(1);
    expect(compareVersions("1.0.0-alpha", "1.0.0-alpha.1")).toBe(-1);
    expect(compareVersions("1.0.0-alpha.1", "1.0.0-alpha.beta")).toBe(-1);
  });

  test("returns null for unparseable input", () => {
    expect(compareVersions("nope", "0.7.23")).toBeNull();
    expect(compareVersions("0.7.23", "")).toBeNull();
  });
});

describe("releaseFromApi", () => {
  test("extracts and normalizes a release", () => {
    expect(
      releaseFromApi({
        tag_name: "v0.7.24",
        html_url: "https://github.com/Crescent617/yomi/releases/tag/v0.7.24",
        published_at: "2026-07-29T09:22:18Z",
      }),
    ).toEqual({
      version: "0.7.24",
      url: "https://github.com/Crescent617/yomi/releases/tag/v0.7.24",
      published_at: "2026-07-29T09:22:18Z",
    });
  });

  test("tolerates a missing publish timestamp", () => {
    const release = releaseFromApi({
      tag_name: "v0.7.24",
      html_url: "https://example.com",
    });
    expect(release?.version).toBe("0.7.24");
    expect(release?.published_at).toBeNull();
  });

  test("rejects malformed payloads", () => {
    expect(releaseFromApi(null)).toBeNull();
    expect(releaseFromApi({})).toBeNull();
    expect(releaseFromApi({ tag_name: 42, html_url: "x" })).toBeNull();
    expect(
      releaseFromApi({ tag_name: "not-a-version", html_url: "x" }),
    ).toBeNull();
  });
});
