import { describe, expect, test } from "vitest";
import {
  blockPuaInput,
  containsPua,
  detectLang,
  isDeliberateRenameExit,
  projectColor,
  sanitizePuaPaste,
  stripPua,
  stripPuaOnInput,
} from "./utils";

describe("projectColor", () => {
  test("is stable for the same project", () => {
    const color = projectColor("Yomi/home/hrli/repos/yomi");

    expect(projectColor("Yomi/home/hrli/repos/yomi")).toBe(color);
  });

  test("uses the semantic muted foreground theme color", () => {
    expect(projectColor("project-a")).toContain("var(--muted-foreground)");
  });

  test("uses the seed to vary project hues", () => {
    expect(projectColor("project-a")).not.toBe(projectColor("project-b"));
  });
});

describe("detectLang", () => {
  test("detects languages independently for multiple file paths", () => {
    expect(detectLang("src/first.ts")).toBe("typescript");
    expect(detectLang("src/second.rs")).toBe("rust");
    expect(detectLang("config/settings.json")).toBe("json");
  });

  test("detects extensionless well-known files by basename", () => {
    expect(detectLang("docker/Dockerfile")).toBe("dockerfile");
    expect(detectLang("Makefile")).toBe("makefile");
  });
});

describe("PUA guard", () => {
  test("containsPua / stripPua cover the full private use area", () => {
    expect(containsPua("plain text 你好")).toBe(false);
    expect(containsPua("arrow\uf700 leak")).toBe(true);
    expect(containsPua("\ue000\uf8ff")).toBe(true);
    expect(stripPua("a\uf700b\uf704c")).toBe("abc");
    expect(stripPua("clean")).toBe("clean");
  });

  test("blockPuaInput blocks every insert* carrying PUA, paste excepted", () => {
    const blocked = (inputType: string, data: string | null) => {
      let prevented = false;
      blockPuaInput({
        inputType,
        data,
        preventDefault: () => (prevented = true),
      } as unknown as InputEvent);
      return prevented;
    };
    expect(blocked("insertText", "\uf700")).toBe(true);
    expect(blocked("insertCompositionText", "\uf701")).toBe(true);
    // 系统文本替换/拖拽/Yank 等冷门插入路径同样拦截。
    expect(blocked("insertReplacementText", "\uf703")).toBe(true);
    expect(blocked("insertFromDrop", "\uf700")).toBe(true);
    expect(blocked("insertText", "a")).toBe(false);
    expect(blocked("insertText", null)).toBe(false);
    // 粘贴不整体拦截 —— 由 sanitizePuaPaste 消毒后插入。
    expect(blocked("insertFromPaste", "\uf700")).toBe(false);
    expect(blocked("deleteContentBackward", "\uf700")).toBe(false);
  });

  test("stripPuaOnInput strips PUA in place and re-bases the caret", () => {
    let selection: [number, number] | null = null;
    const el = {
      value: "ab\uf703cd\uf700",
      selectionStart: 5,
      setSelectionRange: (start: number, end: number) => {
        selection = [start, end];
      },
    } as unknown as HTMLTextAreaElement;
    stripPuaOnInput({ target: el } as unknown as Event);
    expect(el.value).toBe("abcd");
    // 光标前剥了 1 个（位置 2 的 \uf703），5 → 4。
    expect(selection).toEqual([4, 4]);
  });

  test("stripPuaOnInput preserves a multi-char selection across the strip", () => {
    let selection: [number, number] | null = null;
    const el = {
      value: "ab\uf703cdef",
      selectionStart: 1,
      selectionEnd: 6,
      setSelectionRange: (s: number, e: number) => {
        selection = [s, e];
      },
    } as unknown as HTMLTextAreaElement;
    stripPuaOnInput({ target: el } as unknown as Event);
    expect(el.value).toBe("abcdef");
    // 选区双端各自折算：1→1（前无剥除），6→5（前剥 1 个），选区保留。
    expect(selection).toEqual([1, 5]);
  });

  test("stripPuaOnInput survives input types whose selectionStart throws", () => {
    const el = {
      value: "x\uf700y",
      get selectionStart(): number {
        throw new DOMException("InvalidStateError");
      },
      setSelectionRange: () => {
        throw new DOMException("InvalidStateError");
      },
    } as unknown as HTMLInputElement;
    stripPuaOnInput({ target: el } as unknown as Event);
    expect(el.value).toBe("xy");
  });

  test("stripPuaOnInput leaves clean values and composing events alone", () => {
    const clean = {
      value: "plain",
      selectionStart: 2,
      setSelectionRange: () => {
        throw new Error("must not touch selection");
      },
    } as unknown as HTMLInputElement;
    stripPuaOnInput({ target: clean } as unknown as Event);
    expect(clean.value).toBe("plain");

    const composing = {
      value: "a\uf700",
      selectionStart: 2,
      setSelectionRange: () => {},
    } as unknown as HTMLTextAreaElement;
    stripPuaOnInput({
      target: composing,
      isComposing: true,
    } as unknown as Event);
    expect(composing.value).toBe("a\uf700");
  });

  test("sanitizePuaPaste inserts stripped text at the selection and emits input", () => {
    const inserted: unknown[][] = [];
    const events: string[] = [];
    const textarea = {
      selectionStart: 2,
      selectionEnd: 4,
      setRangeText: (...args: unknown[]) => inserted.push(args),
      dispatchEvent: (e: Event) => events.push(e.type),
    } as unknown as HTMLTextAreaElement;
    let prevented = false;
    const event = {
      clipboardData: { getData: () => "ab\uf700cd" },
      preventDefault: () => (prevented = true),
    } as unknown as ClipboardEvent;

    sanitizePuaPaste(event, textarea);

    expect(prevented).toBe(true);
    expect(inserted).toEqual([["abcd", 2, 4, "end"]]);
    expect(events).toEqual(["input"]);
  });

  test("sanitizePuaPaste leaves clean pastes to the browser default", () => {
    let touched = false;
    const textarea = {
      setRangeText: () => (touched = true),
      dispatchEvent: () => (touched = true),
    } as unknown as HTMLTextAreaElement;
    let prevented = false;
    const event = {
      clipboardData: { getData: () => "clean text" },
      preventDefault: () => (prevented = true),
    } as unknown as ClipboardEvent;

    sanitizePuaPaste(event, textarea);

    expect(prevented).toBe(false);
    expect(touched).toBe(false);
  });
});

describe("isDeliberateRenameExit", () => {
  const blur = (relatedTarget: EventTarget | null) =>
    ({ relatedTarget }) as unknown as FocusEvent;

  test("focus shift (Tab/click) is deliberate", () => {
    expect(isDeliberateRenameExit(blur({} as EventTarget), false)).toBe(true);
  });

  test("an armed outside pointerdown is deliberate", () => {
    expect(isDeliberateRenameExit(blur(null), true)).toBe(true);
  });

  test("a keyed-list DOM move (null relatedTarget, no arm) is spurious", () => {
    expect(isDeliberateRenameExit(blur(null), false)).toBe(false);
  });
});
