import { describe, expect, test } from "vitest";
import {
  extractTarget,
  parsePostMessageArgs,
  postMessageSessionTarget,
} from "./tool-utils";

const argumentsJson = JSON.stringify({
  agent_id: "sub_123",
  title: "Review complete",
  content: "Found two issues.",
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
