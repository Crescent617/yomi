import { describe, expect, test } from "vitest";
import {
  extractTarget,
  extraMeta,
  humanizeToolName,
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
    ["web_search", { query: "Svelte runes" }, "Web search", "Svelte runes"],
    [
      "ask_user",
      { questions: [{ question: "Continue?" }] },
      "Ask user",
      "Continue?",
    ],
    ["task_create", { subject: "Ship release" }, "Create task", "Ship release"],
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
    expect(extraMeta("ask_user", JSON.stringify({ questions: [{}, {}] }))).toBe(
      "2 questions",
    );
  });
});

describe("post_message tool rendering", () => {
  test("uses the recipient as the compact target", () => {
    expect(extractTarget("post_message", argumentsJson)).toBe("sub_123");
  });

  test("parses the specialized message fields", () => {
    expect(parsePostMessageArgs(argumentsJson)).toEqual({
      agent_id: "sub_123",
      title: "Review complete",
      content: "Found two issues.",
    });
  });

  test("uses the post_message recipient as its session target", () => {
    expect(postMessageSessionTarget("post_message", argumentsJson)).toBe(
      "sub_123",
    );
    expect(postMessageSessionTarget("read", argumentsJson)).toBeNull();
    expect(postMessageSessionTarget("post_message", "{}")).toBeNull();
  });

  test("rejects incomplete arguments", () => {
    expect(parsePostMessageArgs('{"agent_id":"sub_123"}')).toBeNull();
  });
});

describe("humanizeToolName", () => {
  test.each([
    ["web_search", "WebSearch"],
    ["my_custom_tool", "MyCustomTool"],
    ["my-custom-tool", "MyCustomTool"],
    ["read", "Read"],
    ["webSearch", "WebSearch"],
    ["WebSearch", "WebSearch"],
    ["", ""],
  ])("humanizes %s to %s", (name, expected) => {
    expect(humanizeToolName(name)).toBe(expected);
  });

  test("toolLabel falls back to humanized name for unknown tools", () => {
    expect(toolLabel("my_custom_tool")).toBe("MyCustomTool");
    expect(toolLabel("")).toBe("Tool");
  });
});
