import { describe, expect, test } from "vitest";
import {
  extractTarget,
  extraMeta,
  parsePostMessageArgs,
  postMessageSessionTarget,
  toolLabel,
} from "./tool-utils";

const argumentsJson = JSON.stringify({
  agent_id: "sub_123",
  title: "Review complete",
  content: "Found two issues.",
});

describe("tool header summaries", () => {
  test.each([
    ["read_file", { path: "src/main.rs" }, "Read", "src/main.rs"],
    ["webSearch", { query: "Svelte runes" }, "Web search", "Svelte runes"],
    [
      "askUser",
      { questions: [{ question: "Continue?" }] },
      "Ask user",
      "Continue?",
    ],
    ["taskCreate", { subject: "Ship release" }, "Create task", "Ship release"],
    [
      "send_message",
      { content: "Build finished" },
      "Send message",
      "Build finished",
    ],
  ])("summarizes %s", (name, args, label, target) => {
    expect(toolLabel(name)).toBe(label);
    expect(extractTarget(name, JSON.stringify(args))).toBe(target);
  });

  test("shows useful secondary metadata", () => {
    expect(
      extraMeta(
        "shell",
        JSON.stringify({
          command: "cargo test",
          background: true,
          timeout: 120,
        }),
      ),
    ).toBe("async · timeout 120s");
    expect(extraMeta("askUser", JSON.stringify({ questions: [{}, {}] }))).toBe(
      "2 questions",
    );
  });
});

describe("postMessage tool rendering", () => {
  test("uses the recipient as the compact target", () => {
    expect(extractTarget("postMessage", argumentsJson)).toBe("sub_123");
  });

  test("parses the specialized message fields", () => {
    expect(parsePostMessageArgs(argumentsJson)).toEqual({
      agent_id: "sub_123",
      title: "Review complete",
      content: "Found two issues.",
    });
  });

  test("uses the postMessage recipient as its session target", () => {
    expect(postMessageSessionTarget("postMessage", argumentsJson)).toBe(
      "sub_123",
    );
    expect(postMessageSessionTarget("read", argumentsJson)).toBeNull();
    expect(postMessageSessionTarget("postMessage", "{}")).toBeNull();
  });

  test("rejects incomplete arguments", () => {
    expect(parsePostMessageArgs('{"agent_id":"sub_123"}')).toBeNull();
  });
});
