import { describe, expect, it } from "vitest";
import {
  filterPaletteCommands,
  filterPaletteSessions,
  type PaletteCommand,
} from "./command-palette.svelte";
import type { SessionState } from "./state.svelte";

function cmd(
  partial: Partial<PaletteCommand> & { id: string },
): PaletteCommand {
  return {
    group: "会话",
    title: partial.id,
    keywords: "",
    icon: (() => null) as never,
    run: () => {},
    ...partial,
  };
}

describe("filterPaletteCommands", () => {
  const commands = [
    cmd({ id: "fork", title: "Fork 当前会话", keywords: "fork 分支" }),
    cmd({ id: "restart", title: "重启 Kernel…", keywords: "restart 重启" }),
    cmd({ id: "hidden", title: "隐身命令", enabled: () => false }),
  ];

  it("drops disabled commands", () => {
    const out = filterPaletteCommands(commands, "");
    expect(out.map((c) => c.id)).toEqual(["fork", "restart"]);
  });

  it("matches on keywords as well as title", () => {
    const out = filterPaletteCommands(commands, "restart");
    expect(out.map((c) => c.id)).toEqual(["restart"]);
  });

  it("matches english keywords against a Chinese-only query string", () => {
    const bilingual = [
      cmd({ id: "new", title: "新建会话", keywords: "new session create" }),
    ];
    expect(filterPaletteCommands(bilingual, "new").map((c) => c.id)).toEqual([
      "new",
    ]);
    expect(filterPaletteCommands(bilingual, "新建").map((c) => c.id)).toEqual([
      "new",
    ]);
  });

  it("matches CJK title text", () => {
    const out = filterPaletteCommands(commands, "重启");
    expect(out.map((c) => c.id)).toEqual(["restart"]);
  });
});

describe("filterPaletteSessions", () => {
  function session(
    id: string,
    alias: string,
    updated_at: string,
  ): SessionState {
    return {
      id,
      alias,
      updated_at,
      project_path: `/home/u/${id}`,
    } as SessionState;
  }

  const sessions = [
    session("s1", "OCT meta 返修", "2026-08-28T10:00:00Z"),
    session("s2", "GUI 改版", "2026-08-29T09:00:00Z"),
    session("s3", "看板设计", "2026-08-29T11:00:00Z"),
  ];

  it("empty query: most-recently-updated first", () => {
    const out = filterPaletteSessions(sessions, "");
    expect(out.map((s) => s.id)).toEqual(["s3", "s2", "s1"]);
  });

  it("fuzzy match on alias", () => {
    const out = filterPaletteSessions(sessions, "meta");
    expect(out.map((s) => s.id)).toEqual(["s1"]);
  });

  it("basename keeps only the last path segment", async () => {
    const { basename } = await import("./command-palette.svelte");
    expect(basename("/Users/hrli/repos/yomi")).toBe("yomi");
    expect(basename("~/work/jmir")).toBe("jmir");
    expect(basename("")).toBe("");
    expect(basename("/")).toBe("");
  });

  it("matches by session id substring, not only alias", () => {
    const out = filterPaletteSessions(sessions, "s2");
    expect(out.map((s) => s.id)).toEqual(["s2"]);
  });

  it("matches by id tail", () => {
    const withUlid = [
      session(
        "sess_01M163T2R96BRSVZFPXJ17QQ9S",
        "命令面板",
        "2026-08-29T12:00:00Z",
      ),
    ];
    expect(filterPaletteSessions(withUlid, "17QQ9S").map((s) => s.id)).toEqual([
      "sess_01M163T2R96BRSVZFPXJ17QQ9S",
    ]);
  });

  it("no match → empty list", () => {
    expect(filterPaletteSessions(sessions, "zzz")).toEqual([]);
  });
});
