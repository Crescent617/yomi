import { describe, expect, test } from "vitest";
import { parseSteerMessage } from "./steer-message";

describe("parseSteerMessage", () => {
  test("extracts the kernel From Agent prefix", () => {
    expect(
      parseSteerMessage("[From Agent: sub_123] Finished reviewing the code."),
    ).toEqual({
      source: { type: "agent", id: "sub_123" },
      content: "Finished reviewing the code.",
    });
  });

  test("extracts a background shell source", () => {
    expect(
      parseSteerMessage(
        "[From Shell: sh_123] [Task sh_123 completed]\nOutput: /tmp/sh.log",
      ),
    ).toEqual({
      source: { type: "shell", id: "sh_123" },
      content: "[Task sh_123 completed]\nOutput: /tmp/sh.log",
    });
  });

  test("accepts whitespace and casing around the prefix", () => {
    expect(parseSteerMessage("  [FROM AGENT:  sess_parent  ]\nUpdate")).toEqual(
      {
        source: { type: "agent", id: "sess_parent" },
        content: "Update",
      },
    );
  });

  test("keeps the legacy agent_id prefix readable for stored sessions", () => {
    expect(parseSteerMessage("[agent_id: old_sub] Historical update")).toEqual({
      source: { type: "agent", id: "old_sub" },
      content: "Historical update",
    });
  });

  test("does not parse embedded or malformed prefixes", () => {
    expect(parseSteerMessage("Reply to [From Agent: sub_123] later")).toEqual({
      source: null,
      content: "Reply to [From Agent: sub_123] later",
    });
    expect(parseSteerMessage("[From Agent: ] message")).toEqual({
      source: null,
      content: "[From Agent: ] message",
    });
  });
});
