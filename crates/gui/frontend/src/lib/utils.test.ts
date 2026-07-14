import { describe, expect, test } from "vitest";
import { detectLang } from "./utils";

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
