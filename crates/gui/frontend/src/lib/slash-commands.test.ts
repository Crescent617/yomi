import { describe, expect, test, vi } from "vitest";

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    cancelSession: vi.fn(async () => {}),
    clearSession: vi.fn(async () => {}),
    setPermissionLevel: vi.fn(async () => {}),
    compactSession: vi.fn(async () => {}),
    continueSession: vi.fn(async () => {}),
    sendSteer: vi.fn(async () => {}),
    getCheckpoints: vi.fn(async () => [] as { message_id: string }[]),
    rewind: vi.fn(async () => {}),
  };
});

import * as api from "./api";
import { matchSlashCommands, runSessionCommand } from "./slash-commands";

describe("matchSlashCommands", () => {
  test("empty query returns the whole list", () => {
    const all = matchSlashCommands("");
    expect(all.length).toBeGreaterThan(0);
    expect(all.map(([cmd]) => cmd)).toContain("/clear");
  });

  test("filters by substring", () => {
    const cmds = matchSlashCommands("cl").map(([cmd]) => cmd);
    expect(cmds).toContain("/clear");
    expect(cmds).not.toContain("/yolo");
  });
});

describe("runSessionCommand routing", () => {
  test("unknown command is left to the caller as a normal message", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/new")).toBe(
      "message",
    );
    expect(await runSessionCommand({ session_id: "s1" }, "just text")).toBe(
      "message",
    );
  });

  test("/history without a picker falls through to a normal message", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/history")).toBe(
      "message",
    );
  });

  test("/history with a picker calls it and reports handled", async () => {
    const onHistory = vi.fn();
    expect(
      await runSessionCommand({ session_id: "s1", onHistory }, "/history"),
    ).toBe("handled");
    expect(onHistory).toHaveBeenCalledOnce();
  });

  test("/btw delegates to onBtw when the caller provides one", async () => {
    const onBtw = vi.fn();
    expect(
      await runSessionCommand({ session_id: "s1", onBtw }, "/btw why so"),
    ).toBe("handled");
    expect(onBtw).toHaveBeenCalledWith("why so");
  });
});

describe("runSessionCommand execution", () => {
  test("/cancel runs on the bound session", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/cancel")).toBe(
      "handled",
    );
    expect(vi.mocked(api.cancelSession)).toHaveBeenCalledWith("s1");
  });

  test("/steer wires the caller's compose, blocks and image count", async () => {
    const composeText = vi.fn((t: string) => `Q:${t}`);
    const steerBlocks = vi.fn((t: string) => [{ type: "text", text: t }]);
    expect(
      await runSessionCommand(
        {
          session_id: "s1",
          composeText,
          steerBlocks,
          steerImageCount: 2,
        },
        "/steer look here",
      ),
    ).toBe("handled");
    expect(composeText).toHaveBeenCalledWith("look here");
    expect(steerBlocks).toHaveBeenCalledWith("Q:look here");
    expect(vi.mocked(api.sendSteer)).toHaveBeenCalledWith("s1", [
      { type: "text", text: "Q:look here" },
    ]);
  });

  test("/steer with no content fails so the caller keeps the draft", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/steer")).toBe(
      "failed",
    );
  });

  test("/undo with no checkpoint fails", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/undo")).toBe(
      "failed",
    );
  });

  test("/debug with a non-noti subcommand fails", async () => {
    expect(await runSessionCommand({ session_id: "s1" }, "/debug foo")).toBe(
      "failed",
    );
  });
});
