import { describe, expect, test, vi } from "vitest";
import { createSlashCompletion } from "./slash-completion.svelte";

type KeyStub = KeyboardEvent & { preventDefault: ReturnType<typeof vi.fn> };

// vitest 跑在 node 环境（无 DOM）：补全只读 event.key 并调用 preventDefault，
// 用带 spy 的最小替身即可，顺便断言「导航键确实 preventDefault」。
function key(k: string): KeyStub {
  return { key: k, preventDefault: vi.fn() } as unknown as KeyStub;
}

describe("createSlashCompletion", () => {
  test("shows and filters after a leading slash", () => {
    const c = createSlashCompletion();
    c.detect("/");
    expect(c.show).toBe(true);
    expect(c.commands.length).toBeGreaterThan(0);
    c.detect("/cle");
    expect(c.commands.map(([cmd]) => cmd)).toEqual(["/clear"]);
    expect(c.selected).toBe(0);
  });

  test("hides on non-slash, illegal chars, and after a space", () => {
    const c = createSlashCompletion();
    c.detect("/x");
    expect(c.show).toBe(true);
    c.detect("hello");
    expect(c.show).toBe(false);
    c.detect("/cl ear");
    expect(c.show).toBe(false);
    c.detect("/clear ");
    expect(c.show).toBe(false);
  });

  test("arrow keys move the selection with wraparound", () => {
    const c = createSlashCompletion();
    c.detect("/");
    const n = c.commands.length;
    const up = key("ArrowUp");
    expect(c.keydown(up).consumed).toBe(true);
    expect(up.preventDefault).toHaveBeenCalledOnce();
    expect(c.selected).toBe(n - 1);
    const down = key("ArrowDown");
    expect(c.keydown(down).consumed).toBe(true);
    expect(down.preventDefault).toHaveBeenCalledOnce();
    expect(c.selected).toBe(0);
  });

  test("Tab/Enter accept the selected command and consume the key", () => {
    const c = createSlashCompletion();
    c.detect("/cle");
    const enter = key("Enter");
    expect(c.keydown(enter)).toEqual({
      consumed: true,
      accept: "/clear",
    });
    expect(enter.preventDefault).toHaveBeenCalledOnce();
    const tab = key("Tab");
    expect(c.keydown(tab)).toEqual({
      consumed: true,
      accept: "/clear",
    });
    expect(tab.preventDefault).toHaveBeenCalledOnce();
  });

  test("Escape dismisses and hides", () => {
    const c = createSlashCompletion();
    c.detect("/");
    const esc = key("Escape");
    expect(c.keydown(esc)).toEqual({
      consumed: true,
      dismissed: true,
    });
    // default 由调用方决定（小窗需要它来 stopPropagation），状态机不代劳。
    expect(esc.preventDefault).not.toHaveBeenCalled();
    expect(c.show).toBe(false);
  });

  test("keys pass through when the menu is hidden", () => {
    const c = createSlashCompletion();
    expect(c.keydown(key("Enter"))).toEqual({ consumed: false });
  });
});
