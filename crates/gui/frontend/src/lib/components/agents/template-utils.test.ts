import { describe, expect, it } from "vitest";
import {
  checkTemplateName,
  createDraftDirty,
  NAME_RE,
  NEW_TEMPLATE_STUB,
} from "./template-utils";

const existing = [
  { name: "planner", source: "builtin" },
  { name: "my-critic", source: "global" },
  { name: "release-checker", source: "workspace" },
];

describe("NAME_RE", () => {
  it("accepts kebab-case names", () => {
    for (const name of ["planner", "my-role", "a", "0x"]) {
      expect(NAME_RE.test(name)).toBe(true);
    }
  });

  it("rejects invalid names (parity with kernel validate_name)", () => {
    for (const name of [
      "",
      "-lead",
      "UPPER",
      "has space",
      "../escape",
      "a/b",
    ]) {
      expect(NAME_RE.test(name)).toBe(false);
    }
    expect(NAME_RE.test("x".repeat(65))).toBe(false);
    expect(NAME_RE.test("x".repeat(64))).toBe(true);
  });
});

describe("checkTemplateName", () => {
  it("flags malformed names before any conflict check", () => {
    const r = checkTemplateName("Bad Name", existing, "global");
    expect(r.error).toContain("kebab-case");
    expect(r.override).toBe("");
  });

  it("rejects same-scope duplicates", () => {
    const r = checkTemplateName("my-critic", existing, "global");
    expect(r.error).toContain("already exists in global");
  });

  it("reports an override when the name lives in a lower layer", () => {
    expect(checkTemplateName("planner", existing, "global")).toEqual({
      error: "",
      override: "Will override the builtin template",
    });
    // same name in a *higher* layer: creating in global is shadowed by workspace,
    // but the check only blocks same-scope, so it reports the override note.
    expect(checkTemplateName("release-checker", existing, "global").error).toBe(
      "",
    );
  });

  it("passes clean names", () => {
    expect(checkTemplateName("fresh-role", existing, "global")).toEqual({
      error: "",
      override: "",
    });
  });
});

describe("createDraftDirty", () => {
  it("is clean for the untouched form", () => {
    expect(createDraftDirty("", NEW_TEMPLATE_STUB)).toBe(false);
  });

  it("is dirty once name or body changed", () => {
    expect(createDraftDirty("x", NEW_TEMPLATE_STUB)).toBe(true);
    expect(createDraftDirty("", "edited")).toBe(true);
  });
});
