import { describe, expect, test } from "vitest";
import { resolveBoundSessionId } from "./companion.svelte";

describe("resolveBoundSessionId", () => {
  test("pinned session wins over override and follow", () => {
    expect(
      resolveBoundSessionId({
        pinned_session_id: "pin",
        override_session_id: "ovr",
        active_session_id: "act",
      }),
    ).toBe("pin");
  });

  test("override beats follow when nothing is pinned", () => {
    expect(
      resolveBoundSessionId({
        pinned_session_id: null,
        override_session_id: "ovr",
        active_session_id: "act",
      }),
    ).toBe("ovr");
  });

  test("falls back to the main window active session", () => {
    expect(
      resolveBoundSessionId({
        pinned_session_id: null,
        override_session_id: null,
        active_session_id: "act",
      }),
    ).toBe("act");
  });

  test("null when there is nothing to bind", () => {
    expect(
      resolveBoundSessionId({
        pinned_session_id: null,
        override_session_id: null,
        active_session_id: null,
      }),
    ).toBeNull();
  });
});
