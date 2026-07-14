import { describe, expect, test } from "vitest";
import { parseSteerMessage } from "./steer-message";

describe("parseSteerMessage", () => {
  test("extracts the kernel agent prefix", () => {
    expect(
      parseSteerMessage("[agent_id: sub_123] Finished reviewing the code."),
    ).toEqual({
      agentId: "sub_123",
      content: "Finished reviewing the code.",
    });
  });

  test("accepts whitespace and casing around the prefix", () => {
    expect(parseSteerMessage("  [Agent_ID:  sess_parent  ]\nUpdate")).toEqual({
      agentId: "sess_parent",
      content: "Update",
    });
  });

  test("does not parse embedded or malformed prefixes", () => {
    expect(parseSteerMessage("Reply to [agent_id: sub_123] later")).toEqual({
      agentId: null,
      content: "Reply to [agent_id: sub_123] later",
    });
    expect(parseSteerMessage("[agent_id: ] message")).toEqual({
      agentId: null,
      content: "[agent_id: ] message",
    });
  });
});
