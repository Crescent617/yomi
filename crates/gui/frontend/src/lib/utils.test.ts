import { describe, expect, test } from "vitest";
import { detectLang, projectColor } from "./utils";

describe("projectColor", () => {
  test("is stable for the same project", () => {
    const color = projectColor("Yomi/home/hrli/repos/yomi");

    expect(projectColor("Yomi/home/hrli/repos/yomi")).toBe(color);
  });

  test("uses the semantic muted foreground theme color", () => {
    expect(projectColor("project-a")).toContain("var(--muted-foreground)");
  });

  test("uses the seed to vary project hues", () => {
    expect(projectColor("project-a")).not.toBe(projectColor("project-b"));
  });
});

describe("detectLang", () => {
  test("detects languages independently for multiple file paths", () => {
    expect(detectLang("src/first.ts")).toBe("typescript");
    expect(detectLang("src/second.rs")).toBe("rust");
    expect(detectLang("config/settings.json")).toBe("json");
  });

  test("detects extensionless well-known files by basename", () => {
    expect(detectLang("docker/Dockerfile")).toBe("dockerfile");
    expect(detectLang("Makefile")).toBe("makefile");
  });
});
