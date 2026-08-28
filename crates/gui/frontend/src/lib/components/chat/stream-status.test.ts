import { describe, expect, test } from "vitest";
import {
  extractPartialTarget,
  formatStreamTokens,
  formatTapeElapsed,
  toolVerb,
} from "./stream-status";

describe("formatStreamTokens", () => {
  test("prefixes small counts with ~ and pluralizes", () => {
    expect(formatStreamTokens(1)).toBe("~1 token");
    expect(formatStreamTokens(42)).toBe("~42 tokens");
  });

  test("collapses thousands to one decimal", () => {
    expect(formatStreamTokens(1200)).toBe("~1.2k tokens");
    expect(formatStreamTokens(10500)).toBe("~10.5k tokens");
  });
});

describe("toolVerb", () => {
  test("maps known tools to present-tense verbs", () => {
    expect(toolVerb("edit")).toBe("Editing");
    expect(toolVerb("read")).toBe("Reading");
    expect(toolVerb("write")).toBe("Writing");
    expect(toolVerb("shell")).toBe("Running");
    expect(toolVerb("grep")).toBe("Finding");
    expect(toolVerb("agent")).toBe("Delegating");
    expect(toolVerb("todo")).toBe("Planning");
    expect(toolVerb("ask_user")).toBe("Asking");
    expect(toolVerb("post_message")).toBe("Messaging");
  });

  test("falls back to Calling for unknown tools", () => {
    expect(toolVerb("mcp__something")).toBe("Calling");
  });

  test("shell verbs follow the command being run", () => {
    expect(toolVerb("shell", '{"command": "cargo build --release"}')).toBe(
      "Building",
    );
    expect(toolVerb("shell", '{"command": "npm test"}')).toBe("Testing");
    expect(toolVerb("shell", '{"command": "npm install"}')).toBe("Installing");
    expect(toolVerb("shell", '{"command": "git push origin main"}')).toBe(
      "Pushing",
    );
    expect(toolVerb("shell", '{"command": "git diff --stat"}')).toBe(
      "Inspecting",
    );
    expect(toolVerb("shell", '{"command": "rg pattern src/"}')).toBe(
      "Searching",
    );
    expect(toolVerb("shell", '{"command": "ls -la"}')).toBe("Exploring");
    expect(toolVerb("shell", '{"command": "curl -s example.com"}')).toBe(
      "Fetching",
    );
    expect(toolVerb("shell", '{"command": "some-custom-tool --flag"}')).toBe(
      "Running",
    );
  });

  test("shell verbs work with compound and partial commands", () => {
    expect(toolVerb("shell", '{"command": "cargo test && echo done"}')).toBe(
      "Testing",
    );
    // Arguments still streaming (truncated JSON) still resolve, as long as
    // the command word itself is complete.
    expect(toolVerb("shell", '{"command": "cargo build --rel')).toBe(
      "Building",
    );
  });
});

describe("extractPartialTarget", () => {
  test("extracts from complete JSON via the strict parser", () => {
    expect(
      extractPartialTarget(
        "edit",
        '{"path": "a.rs", "old_str": "x", "new_str": "y"}',
      ),
    ).toBe("a.rs");
    expect(extractPartialTarget("shell", '{"command": "cargo build"}')).toBe(
      "cargo build",
    );
  });

  test("extracts from truncated JSON while later args still stream", () => {
    expect(
      extractPartialTarget("edit", '{"path": "crates/a.rs", "old_str": "fn ma'),
    ).toBe("crates/a.rs");
  });

  test("picks up a target value whose own string is unterminated", () => {
    expect(extractPartialTarget("read", '{"path": "crates/gu')).toBe(
      "crates/gu",
    );
  });

  test("uses the first target key in argument order", () => {
    expect(
      extractPartialTarget(
        "write",
        '{"file_path": "out.md", "content": "# hi"}',
      ),
    ).toBe("out.md");
  });

  test("returns empty for unknown tools or missing keys", () => {
    expect(extractPartialTarget("mcp__x", '{"path": "a"}')).toBe("");
    expect(extractPartialTarget("edit", "")).toBe("");
    expect(extractPartialTarget("edit", "{}")).toBe("");
  });
});

describe("formatTapeElapsed", () => {
  test("bare seconds under a minute", () => {
    expect(formatTapeElapsed(0)).toBe("0s");
    expect(formatTapeElapsed(8)).toBe("8s");
    expect(formatTapeElapsed(59.9)).toBe("59s");
  });

  test("NmNs under an hour", () => {
    expect(formatTapeElapsed(60)).toBe("1m0s");
    expect(formatTapeElapsed(84)).toBe("1m24s");
  });

  test("NhNmNs beyond an hour", () => {
    expect(formatTapeElapsed(3600)).toBe("1h0m0s");
    expect(formatTapeElapsed(3753)).toBe("1h2m33s");
  });
});
